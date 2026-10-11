//! A real OTLP/HTTPS export: ring negotiates TLS and validates a generated root.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use opentelemetry::metrics::MeterProvider;
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, Temporality};
use prost::Message;
use rustls::pki_types::PrivatePkcs8KeyDer;

#[test]
fn otlp_https_exports_and_rejects_invalid_server_identity() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .unwrap();
    for (hostname, trusted, succeeds) in [
        ("localhost", true, true),
        ("localhost", false, false),
        ("unrelated.invalid", true, false),
    ] {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec![hostname.to_owned()]).unwrap();
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone()],
                PrivatePkcs8KeyDer::from(signing_key.serialize_der()).into(),
            )
            .unwrap();
        let (endpoint, receiver) = receiver(config);

        // Certificate and hostname verification stay on, including the negative cases.
        let roots = if trusted {
            vec![reqwest::Certificate::from_der(cert.der()).unwrap()]
        } else {
            vec![]
        };
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .tls_certs_only(roots)
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_http()
            .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
            .with_temporality(Temporality::Delta)
            .with_endpoint(endpoint)
            .with_http_client(client)
            .with_retry_policy(opentelemetry_otlp::RetryPolicy::disabled())
            .build()
            .unwrap();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter).build())
            .build();
        provider
            .meter("tls-test")
            .u64_counter("tls_exports")
            .build()
            .add(1, &[]);
        assert_eq!(
            provider.force_flush().is_ok(),
            succeeds,
            "hostname={hostname}, trusted={trusted}"
        );
        let request = receiver.join().unwrap();
        if succeeds {
            let request = request.unwrap();
            let metric = &request.resource_metrics[0].scope_metrics[0].metrics[0];
            assert_eq!(metric.name, "tls_exports");
            let Some(opentelemetry_proto::tonic::metrics::v1::metric::Data::Sum(sum)) =
                &metric.data
            else {
                panic!("expected a counter sum");
            };
            assert_eq!(
                sum.data_points[0].value,
                Some(opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsInt(1))
            );
        } else {
            assert!(request.is_err(), "TLS receiver must reject the handshake");
        }
        // Delta collection consumed the counter; shutdown has no second export.
        provider.shutdown().unwrap();
    }
}

fn receiver(
    config: rustls::ServerConfig,
) -> (
    String,
    std::thread::JoinHandle<std::io::Result<ExportMetricsServiceRequest>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!(
        "https://localhost:{}/v1/metrics",
        listener.local_addr().unwrap().port()
    );
    let receiver = std::thread::spawn(move || -> std::io::Result<_> {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let connection = rustls::ServerConnection::new(Arc::new(config)).unwrap();
        let mut stream = BufReader::new(rustls::StreamOwned::new(connection, socket));
        let mut line = String::new();
        stream.read_line(&mut line)?;
        assert_eq!(line, "POST /v1/metrics HTTP/1.1\r\n");
        let mut length = None;
        let mut protobuf = false;
        loop {
            line.clear();
            assert!(stream.read_line(&mut line).unwrap() > 0);
            if line == "\r\n" {
                break;
            }
            let (name, value) = line.split_once(':').unwrap();
            if name.eq_ignore_ascii_case("content-length") {
                length = Some(value.trim().parse::<usize>().unwrap());
            }
            if name.eq_ignore_ascii_case("content-type") {
                protobuf = value.trim() == "application/x-protobuf";
            }
        }
        assert!(protobuf);
        let length = length.unwrap();
        assert!(length <= 64 * 1024);
        let mut body = vec![0; length];
        stream.read_exact(&mut body).unwrap();
        let request = ExportMetricsServiceRequest::decode(body.as_slice()).unwrap();
        assert!(!stream.get_ref().conn.is_handshaking());
        assert_eq!(
            stream.get_ref().conn.protocol_version(),
            Some(rustls::ProtocolVersion::TLSv1_3)
        );
        stream.get_mut().write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: application/x-protobuf\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    ).unwrap();
        stream.get_mut().flush().unwrap();
        Ok(request)
    });
    (endpoint, receiver)
}

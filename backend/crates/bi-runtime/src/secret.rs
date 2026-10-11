//! Keeps passwords of libpq connection strings out of logs, `Debug` output
//! and error messages. Understands both libpq forms: `key = value` pairs
//! (optionally single-quoted, with `\` escapes) and `postgres[ql]://` URIs
//! (password in the user info or the query string, percent-encoded).

const MASK: &str = "***";

/// Every password in `conninfo`, both as written and decoded, so callers can
/// remove whichever form a message repeats. `None` when `conninfo` cannot be
/// parsed: nothing derived from it may then be shown.
#[must_use]
pub fn passwords(conninfo: &str) -> Option<Vec<String>> {
    let trimmed = conninfo.trim_start();
    let mut found = if trimmed.starts_with("postgresql://") || trimmed.starts_with("postgres://") {
        uri_passwords(trimmed)?
    } else {
        keyword_value_passwords(conninfo)?
    };
    found.retain(|password| !password.is_empty());
    // Longest first, so a written form is removed before its decoded part.
    found.sort_by_key(|password| std::cmp::Reverse(password.len()));
    found.dedup();
    Some(found)
}

/// `message` with every password of `conninfo` replaced by `***`; a generic
/// text when `conninfo` cannot be parsed.
#[must_use]
pub fn scrub(message: &str, conninfo: &str) -> String {
    match passwords(conninfo) {
        Some(passwords) => passwords.iter().fold(message.to_owned(), |text, password| {
            text.replace(password.as_str(), MASK)
        }),
        None => "details hidden: the connection string could not be parsed".to_owned(),
    }
}

fn keyword_value_passwords(conninfo: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = conninfo.chars().collect();
    let mut i = 0;
    let mut found = Vec::new();
    loop {
        while chars.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
        if i == chars.len() {
            return Some(found);
        }
        let key_start = i;
        while chars
            .get(i)
            .is_some_and(|c| !c.is_whitespace() && *c != '=')
        {
            i += 1;
        }
        let key: String = chars[key_start..i].iter().collect();
        while chars.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
        if chars.get(i) != Some(&'=') || key.is_empty() {
            return None;
        }
        i += 1;
        while chars.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
        let value_start = i;
        let mut value = String::new();
        if chars.get(i) == Some(&'\'') {
            i += 1;
            loop {
                match chars.get(i) {
                    None => return None,
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some('\\') => {
                        value.push(*chars.get(i + 1)?);
                        i += 2;
                    }
                    Some(c) => {
                        value.push(*c);
                        i += 1;
                    }
                }
            }
        } else {
            while let Some(c) = chars.get(i).filter(|c| !c.is_whitespace()) {
                if *c == '\\' {
                    value.push(*chars.get(i + 1)?);
                    i += 2;
                } else {
                    value.push(*c);
                    i += 1;
                }
            }
        }
        if key == "password" {
            found.push(chars[value_start..i].iter().collect());
            found.push(value);
        }
    }
}

fn uri_passwords(uri: &str) -> Option<Vec<String>> {
    let rest = uri.split_once("://")?.1;
    let (before_query, query) = rest.split_once('?').unwrap_or((rest, ""));
    let authority = before_query.split('/').next().unwrap_or_default();
    let mut found = Vec::new();
    if let Some((user_info, _hosts)) = authority.rsplit_once('@')
        && let Some((_user, password)) = user_info.split_once(':')
    {
        found.push(password.to_owned());
        found.push(percent_decode(password)?);
    }
    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=')
            && percent_decode(key)? == "password"
        {
            found.push(value.to_owned());
            found.push(percent_decode(value)?);
        }
    }
    Some(found)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hides(conninfo: &str, secret: &str) {
        let message = format!("connection to {conninfo} failed; password was {secret}");
        let scrubbed = scrub(&message, conninfo);
        assert!(!scrubbed.contains(secret), "{conninfo:?} → {scrubbed}");
    }

    #[test]
    fn keyword_value_forms() {
        hides("dbname=x host=h password=s3cret user=u", "s3cret");
        hides("password = s3cret host=h", "s3cret");
        hides("host=h password='with spaces inside'", "with spaces inside");
        hides("host=h password='it\\'s \\\\ hard'", "it's \\ hard");
        hides("host=h password=back\\ slash", "back slash");
        assert_eq!(
            passwords("host=h password='a b' password=c").unwrap(),
            ["'a b'", "a b", "c"]
        );
    }

    #[test]
    fn uri_forms() {
        hides("postgresql://bi:p%40ss%20word@db:5432/lake", "p@ss word");
        hides(
            "postgresql://bi:p%40ss%20word@db:5432/lake",
            "p%40ss%20word",
        );
        hides("postgres://db/lake?user=bi&password=se%26cret", "se&cret");
        hides("postgres://db/lake?password=plain&sslmode=require", "plain");
    }

    #[test]
    fn unparseable_strings_reveal_nothing() {
        for conninfo in [
            "host=h password='unterminated",
            "password",
            "=x",
            "postgresql://u:%zz@h/d",
        ] {
            assert_eq!(
                scrub("anything s3cret", conninfo),
                "details hidden: the connection string could not be parsed",
                "{conninfo}"
            );
        }
    }

    #[test]
    fn strings_without_passwords_are_left_alone() {
        assert_eq!(
            scrub("could not connect", "host=h dbname=d"),
            "could not connect"
        );
    }
}

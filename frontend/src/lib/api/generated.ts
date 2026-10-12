// Generated from docs/api/openapi.json. Run npm run generate:api.
export interface paths {
  '/api/v1/dev/login': {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    get?: never;
    put?: never;
    /** @description Solo feature dev-auth. Requiere Origin configurado; selecciona identidad sintética sin contraseña. Rota la sesión anterior. */
    post: operations['devLogin'];
    delete?: never;
    options?: never;
    head?: never;
    patch?: never;
    trace?: never;
  };
  '/api/v1/dev/users': {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    /** @description Solo feature dev-auth con BI_DEV_USERS configurado; únicamente fixtures sintéticos. */
    get: operations['getDevUsers'];
    put?: never;
    post?: never;
    delete?: never;
    options?: never;
    head?: never;
    patch?: never;
    trace?: never;
  };
  '/api/v1/logout': {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    get?: never;
    put?: never;
    post: operations['logout'];
    delete?: never;
    options?: never;
    head?: never;
    patch?: never;
    trace?: never;
  };
  '/api/v1/models': {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    /** Recarga metrics/*.yaml, *.yml y *.json del proyecto y compila cada archivo con el catálogo analítico real. */
    get: operations['listModels'];
    put?: never;
    post?: never;
    delete?: never;
    options?: never;
    head?: never;
    patch?: never;
    trace?: never;
  };
  '/api/v1/queries': {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    get?: never;
    put?: never;
    /** Consulta agregada tipada sobre un dataset permitido; sin SQL del cliente. */
    post: operations['runQuery'];
    delete?: never;
    options?: never;
    head?: never;
    patch?: never;
    trace?: never;
  };
  '/api/v1/session': {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    get: operations['getSession'];
    put?: never;
    post?: never;
    delete?: never;
    options?: never;
    head?: never;
    patch?: never;
    trace?: never;
  };
  '/health/live': {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    /** Liveness: el proceso responde. No consulta dependencias. */
    get: operations['getLiveness'];
    put?: never;
    post?: never;
    delete?: never;
    options?: never;
    head?: never;
    patch?: never;
    trace?: never;
  };
  '/health/ready': {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    /** Readiness: PostgreSQL accesible, migraciones aplicadas y, si está habilitado, el lago analítico listo. */
    get: operations['getReadiness'];
    put?: never;
    post?: never;
    delete?: never;
    options?: never;
    head?: never;
    patch?: never;
    trace?: never;
  };
}
export type webhooks = Record<string, never>;
export interface components {
  schemas: {
    /**
     * @description Logical type of a result column. Every non-null value travels as a JSON
     *     string, so `integer` (64- or 128-bit) and `decimal` stay exact in clients.
     * @enum {string}
     */
    ColumnType: 'string' | 'integer' | 'decimal' | 'timestamptz';
    /** @enum {string} */
    DataScope: 'synthetic';
    DevLogin: {
      /** @description Selects a configured synthetic identity; this is not password authentication. */
      user_id: string;
    };
    /** @enum {string} */
    DiagnosticSeverity: 'error' | 'warning';
    Filter: {
      /** @description A dimension of the dataset. */
      field: string;
      op: components['schemas']['FilterOp'];
      /** @description 1 to 100 values; `null` matches missing values. */
      values: (string | null)[];
    };
    /** @enum {string} */
    FilterOp: 'in';
    Health: {
      checks: components['schemas']['HealthCheck'][];
      status: components['schemas']['HealthStatus'];
    };
    HealthCheck: {
      /**
       * @example postgres
       * @example migrations
       */
      name: string;
      status: components['schemas']['HealthStatus'];
    };
    /** @enum {string} */
    HealthStatus: 'ok' | 'degraded' | 'unavailable';
    ModelDiagnostic: {
      code: string;
      /** Format: int64 */
      column?: number | null;
      /** Format: int64 */
      line?: number | null;
      message: string;
      /** @description JSON pointer; empty string denotes the document root. */
      path: string;
      severity: components['schemas']['DiagnosticSeverity'];
    };
    /** @description An invalid file never exposes metadata from a previous successful load. */
    ModelFile:
      | {
          diagnostics: components['schemas']['ModelDiagnostic'][];
          dimensions: components['schemas']['ModelMember'][];
          file: string;
          fingerprint: string;
          metrics: components['schemas']['ModelMember'][];
          name: string;
          /** @enum {string} */
          status: 'valid';
        }
      | {
          diagnostics: components['schemas']['ModelDiagnostic'][];
          file: string;
          name: string;
          /** @enum {string} */
          status: 'invalid';
        };
    ModelMember: {
      /** @description Exact logical type, e.g. INT64, DECIMAL(18,2), TIMESTAMPTZ (instant). */
      data_type: string;
      id: string;
      name: string;
      /**
       * @description Direct dimension binding nullability. Null means not inferred for
       *     computed dimensions or metrics, whose result has not been executed.
       */
      nullable?: boolean | null;
    };
    ModelsResponse: {
      diagnostics: components['schemas']['ModelDiagnostic'][];
      models: components['schemas']['ModelFile'][];
      project_status: components['schemas']['ProjectStatus'];
    };
    Problem: {
      /** @description Stable identifier for clients, e.g. `invalid_query`. */
      code: string;
      /** @description Human-readable explanation, safe to show; never internal errors. */
      detail?: string | null;
      /** @description Correlates with server logs (`x-request-id`). */
      request_id: string;
      /** Format: int32 */
      status: number;
      title: string;
      /**
       * Format: uri
       * @description Problem type URI; `about:blank` means the HTTP status says it all.
       */
      type: string;
    };
    /** @enum {string} */
    ProjectStatus: 'not_configured' | 'ready' | 'unavailable';
    /**
     * @description An aggregate query over an allowlisted dataset. No SQL is accepted.
     * @example {
     *       "dataset": "sales",
     *       "dimensions": [
     *         "region"
     *       ],
     *       "filters": [
     *         {
     *           "field": "region",
     *           "op": "in",
     *           "values": [
     *             "north",
     *             null
     *           ]
     *         }
     *       ],
     *       "limit": 100,
     *       "measures": [
     *         "row_count",
     *         "amount_sum"
     *       ],
     *       "sort": [
     *         {
     *           "direction": "desc",
     *           "field": "amount_sum"
     *         }
     *       ]
     *     }
     */
    QueryRequest: {
      /** @description Dataset name, e.g. `sales`. */
      dataset: string;
      /** @description Group-by fields (at most 2). */
      dimensions?: string[];
      /** @description Conditions on dimensions, combined with AND. */
      filters?: components['schemas']['Filter'][];
      /**
       * Format: int32
       * @description Maximum rows returned, 1 to 10000 (default 1000).
       */
      limit?: number | null;
      /** @description Aggregates to compute (1 to 8). */
      measures: string[];
      /** @description Ordering by selected fields; ties are broken by the dimensions. */
      sort?: components['schemas']['SortKey'][];
    };
    QueryResult: {
      columns: components['schemas']['ResultColumn'][];
      row_count: number;
      /**
       * @description Rows in column order. Every non-null value is a string in its column's
       *     type (`integer` and `decimal` are exact; `null` means missing).
       */
      rows: (string | null)[][];
      /** @description More rows matched than `limit` allowed. */
      truncated: boolean;
    };
    /** @description Describes one column of a [`super::QueryResult`]. */
    ResultColumn: {
      name: string;
      nullable: boolean;
      /**
       * Format: int32
       * @description Present for `decimal`.
       */
      precision?: number | null;
      /**
       * Format: int32
       * @description Present for `decimal`.
       */
      scale?: number | null;
      type: components['schemas']['ColumnType'];
    };
    Session: {
      /** @description Server-derived restriction. This session grants no access to real sources. */
      data_scope: components['schemas']['DataScope'];
      /** Format: date-time */
      expires_at: string;
      user: components['schemas']['User'];
    };
    /** @enum {string} */
    SortDirection: 'asc' | 'desc';
    SortKey: {
      direction?: components['schemas']['SortDirection'];
      /** @description A selected dimension or measure. */
      field: string;
    };
    User: {
      display_name: string;
      id: string;
    };
  };
  responses: never;
  parameters: never;
  requestBodies: never;
  headers: never;
  pathItems: never;
}
export type $defs = Record<string, never>;
export interface operations {
  devLogin: {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    requestBody: {
      content: {
        'application/json': components['schemas']['DevLogin'];
      };
    };
    responses: {
      /** @description Sesión creada; cookie HttpOnly SameSite=Strict y Secure con HTTPS */
      200: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['Session'];
        };
      };
      /** @description Solicitud JSON inválida */
      400: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Usuario no configurado */
      401: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Origin ausente o distinto al configurado */
      403: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Login de desarrollo deshabilitado */
      404: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description No se pudo crear la sesión */
      503: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
    };
  };
  getDevUsers: {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      /** @description Identidades sintéticas configuradas */
      200: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['User'][];
        };
      };
      /** @description Login de desarrollo deshabilitado */
      404: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
    };
  };
  logout: {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      /** @description Sesión revocada y cookie eliminada */
      204: {
        headers: {
          [name: string]: unknown;
        };
        content?: never;
      };
      /** @description Sin sesión válida */
      401: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Origin ausente o distinto al configurado */
      403: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description PostgreSQL no disponible */
      503: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
    };
  };
  listModels: {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      /** @description Estado del proyecto y resultados por archivo; sin configuración, lista vacía. Directorio inaccesible o límites globales: project_status unavailable y diagnóstico; nunca resultados parciales. */
      200: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['ModelsResponse'];
        };
      };
      /** @description Sin sesión válida */
      401: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Demasiadas cargas simultáneas */
      429: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Motor no disponible o lectura fallida */
      503: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Tiempo de compilación agotado */
      504: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
    };
  };
  runQuery: {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    requestBody: {
      content: {
        'application/json': components['schemas']['QueryRequest'];
      };
    };
    responses: {
      /** @description Resultado tipado y exacto; fases en la cabecera `Server-Timing` (plan, queue, execute, serialize; ms) */
      200: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['QueryResult'];
        };
      };
      /** @description JSON inválido o campos desconocidos */
      400: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Sin sesión válida (`session_required`) */
      401: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Origin ausente o distinto al configurado (`origin_forbidden`) */
      403: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Consulta fuera del catálogo o de los límites (`invalid_query`), o resultado mayor que el presupuesto de bytes (`result_too_large`) */
      422: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Cola de consultas llena; reintentar */
      429: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Fallo interno al ejecutar */
      500: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Motor analítico no disponible */
      503: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Superó el tiempo máximo y se canceló */
      504: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
    };
  };
  getSession: {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      /** @description Sesión verificada por el servidor */
      200: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['Session'];
        };
      };
      /** @description Sin sesión válida */
      401: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description PostgreSQL no disponible */
      503: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
    };
  };
  getLiveness: {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      /** @description Proceso activo */
      200: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['Health'];
        };
      };
    };
  };
  getReadiness: {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      /** @description Listo para servir */
      200: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['Health'];
        };
      };
      /** @description No listo; `checks` indica qué dependencia falla */
      503: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['Health'];
        };
      };
    };
  };
}

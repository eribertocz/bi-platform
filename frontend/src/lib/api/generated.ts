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
    /**
     * Recarga metrics/*.yaml, *.yml y *.json del proyecto y compila cada archivo con el catálogo analítico real.
     * @description Requiere `view` sobre `Model::"project:catalog"`: inspección del catálogo completo del proyecto, incluidos archivos inválidos y metadatos confidenciales o restringidos. No concede consultar, visualizar ni extraer datos. La decisión se toma antes de leer `metrics/` y se audita antes de responder.
     */
    get: operations['listModels'];
    put?: never;
    post?: never;
    delete?: never;
    options?: never;
    head?: never;
    patch?: never;
    trace?: never;
  };
  '/api/v1/oidc/callback': {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    /** @description Acción session:login, recurso sesión propia. Requiere state, code y cookie temporal; consume el intento atómicamente antes del intercambio. Solo redirige a /. */
    get: operations['completeOidcLogin'];
    put?: never;
    post?: never;
    delete?: never;
    options?: never;
    head?: never;
    patch?: never;
    trace?: never;
  };
  '/api/v1/oidc/config': {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    /** @description Acción session:login, recurso sesión propia; público, no revela secretos. */
    get: operations['getOidcConfig'];
    put?: never;
    post?: never;
    delete?: never;
    options?: never;
    head?: never;
    patch?: never;
    trace?: never;
  };
  '/api/v1/oidc/start': {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    get?: never;
    put?: never;
    /** @description Acción session:login, recurso sesión propia. Origin obligatorio. PKCE S256, nonce y state con vida de cinco minutos. Redirección fija. */
    post: operations['startOidcLogin'];
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
  '/api/v1/semantic/explain': {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    get?: never;
    put?: never;
    /** Explains a semantic query without executing it. */
    post: operations['explain'];
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
    /**
     * Readiness: PostgreSQL accesible, esquema de metadatos compatible con esta versión, rol de metadatos seguro (`metadata_security`, solo en el modo restringido por defecto) y, si está habilitado, el lago analítico listo.
     * @description En el modo restringido el servidor nunca migra: `migrations` exige que las migraciones las haya aplicado `bi-server migrate` con la credencial administrativa, sin versiones fallidas, desconocidas ni con checksum distinto. Mientras no esté listo, las operaciones de `/api/v1` responden 503 `metadata_not_ready`.
     */
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
    ExplainDecision: {
      /** @description Selections returned as typed `NULL`, flagged as masked. */
      masked: string[];
      /** @description Stable reason code (`permitted`). */
      reason: string;
      /** @description The roles the project grants to the session's user. */
      roles: string[];
    };
    ExplainPlan: {
      /** @description Stable explanation: one line per operator, then type and unit contracts. */
      explanation: string;
      /**
       * Format: int32
       * @description Correctness obligations the certificate discharges.
       */
      obligations: number;
    };
    ExplainRequest: {
      /**
       * @description The project file, as `/api/v1/models` lists it (`metrics/ventas.yaml`).
       * @example metrics/ventas.yaml
       */
      model: string;
      query: components['schemas']['SemanticQuery'];
    };
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
       * @example metadata_security
       */
      name: string;
      status: components['schemas']['HealthStatus'];
    };
    /** @enum {string} */
    HealthStatus: 'ok' | 'degraded' | 'unavailable';
    LoginConfig: {
      enabled: boolean;
    };
    LoginStart: {
      authorization_url: string;
    };
    /** @description Context is descriptive, never executable or a semantic name alias. */
    ModelAiContext: string | components['schemas']['ModelAiContextObject'];
    ModelAiContextObject: {
      examples?: string[] | null;
      instructions?: string | null;
      synonyms?: string[] | null;
    } & {
      [key: string]: unknown;
    };
    ModelAnnotation: {
      ai_context?: components['schemas']['ModelAiContext'] | null;
      /** @description Explicit declaration; null means the datatype supplies the default. */
      declared_is_time?: boolean | null;
      description?: string | null;
      /** @description Effective time role for fields/dimensions; null for a metric. */
      is_time?: boolean | null;
      label?: string | null;
      /** @description Ossie declaration name, useful when a dimension has several origins. */
      name: string;
      /** @description JSON pointer in this Ossie file, identifying the declaration. */
      path: string;
    };
    ModelDataset: {
      ai_context?: components['schemas']['ModelAiContext'] | null;
      description?: string | null;
      /** @description All declared fields, including computed fields that CSMIR inlines. */
      fields: components['schemas']['ModelField'][];
      name: string;
    };
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
    ModelField: {
      annotation: components['schemas']['ModelAnnotation'];
      is_dimension: boolean;
      name: string;
    };
    /** @description An invalid file never exposes metadata from a previous successful load. */
    ModelFile:
      | {
          ai_context?: components['schemas']['ModelAiContext'] | null;
          datasets: components['schemas']['ModelDataset'][];
          description?: string | null;
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
      /** @description Metadata per Ossie origin; conformed dimensions keep every binding. */
      annotations: components['schemas']['ModelAnnotation'][];
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
      /**
       * Format: uri
       * @description Configured application origin, present only for an origin rejection.
       */
      public_origin?: string | null;
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
     *       "measures": [
     *         "row_count",
     *         "amount_sum"
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
     *       "sort": [
     *         {
     *           "field": "amount_sum",
     *           "direction": "desc"
     *         }
     *       ],
     *       "limit": 100
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
    /** @enum {string} */
    SemanticComparison: '=' | '<>' | '<' | '<=' | '>' | '>=';
    /** @description A use the session's roles do not allow. */
    SemanticDenial: components['schemas']['Problem'] & {
      denied?: components['schemas']['SemanticDeniedUse'] | null;
      /** @description `no_permit`, `forbidden`, `evaluation_error` or `invalid_request`. */
      reason: string;
    };
    SemanticDeniedUse: {
      /** @description `metric`, `dimension`, `dataset` or `model`. */
      kind: string;
      /** @description Only for objects the query named; what the plan reads stays unnamed. */
      name?: string | null;
      /** @description `project`, `filter`, `order`, `group`, `join` or `lineage`. */
      usage: string;
    };
    SemanticDiagnostic: {
      /** @description Stable code (§32). */
      code: string;
      message: string;
      /** @description JSON pointer into the request, when the binder knows it. */
      path?: string | null;
      /** @description `binding`, `correctness`, `planning` or `model`. */
      phase: string;
    };
    /** @description The answer: what was decided, the certified plan and its receipt. */
    SemanticExplanation: {
      decision: components['schemas']['ExplainDecision'];
      plan: components['schemas']['ExplainPlan'];
      receipt: components['schemas']['SemanticReceipt'];
    };
    SemanticFilter:
      | {
          comparison: components['schemas']['SemanticComparison'];
          /** @enum {string} */
          op: 'compare';
          target: components['schemas']['SemanticTarget'];
          value: components['schemas']['SemanticValue'];
        }
      | {
          high: components['schemas']['SemanticValue'];
          low: components['schemas']['SemanticValue'];
          /** @enum {string} */
          op: 'between';
          target: components['schemas']['SemanticTarget'];
        }
      | {
          /** @enum {string} */
          op: 'in';
          target: components['schemas']['SemanticTarget'];
          values: components['schemas']['SemanticValue'][];
        }
      | {
          /** @enum {string} */
          op: 'is_null';
          target: components['schemas']['SemanticTarget'];
        }
      | {
          /** @enum {string} */
          op: 'is_not_null';
          target: components['schemas']['SemanticTarget'];
        }
      | {
          filters: components['schemas']['SemanticFilter'][];
          /** @enum {string} */
          op: 'and';
        }
      | {
          filters: components['schemas']['SemanticFilter'][];
          /** @enum {string} */
          op: 'or';
        }
      | {
          filter: components['schemas']['SemanticFilter'];
          /** @enum {string} */
          op: 'not';
        };
    /** @enum {string} */
    SemanticGrain: 'second' | 'minute' | 'hour' | 'day' | 'week' | 'month' | 'quarter' | 'year';
    SemanticOrder: {
      descending?: boolean;
      /** @description Nulls go last unless stated. */
      nulls_first?: boolean;
      /** @description A selection `id` of this query. */
      selection: string;
    };
    SemanticQuery: {
      filter?: components['schemas']['SemanticFilter'] | null;
      having?: components['schemas']['SemanticFilter'] | null;
      /** Format: int64 */
      limit?: number | null;
      /** Format: int64 */
      offset?: number | null;
      order_by?: components['schemas']['SemanticOrder'][];
      /** @description Output columns, in order. The selected dimensions are the grain. */
      selections: components['schemas']['SemanticSelection'][];
    };
    /** @description Fingerprints of everything the answer depended on. */
    SemanticReceipt: {
      /** @description The session values the model's row policies read; never the values. */
      context: string;
      decision: string;
      /** @description Covers every fingerprint below. */
      id: string;
      model: string;
      plan: string;
      policies: string;
      query: string;
    };
    /** @description A query the engine cannot answer as asked, with stable diagnostics. */
    SemanticRejection: components['schemas']['Problem'] & {
      diagnostics: components['schemas']['SemanticDiagnostic'][];
    };
    SemanticSelection:
      | {
          /** @description Query-local name of the column; ordering and `having` use it. */
          id: string;
          /** @enum {string} */
          kind: 'metric';
          label?: string | null;
          /** @description The metric's name in the model. */
          name: string;
        }
      | {
          id: string;
          /** @enum {string} */
          kind: 'dimension';
          label?: string | null;
          name: string;
          /** @description Relationship roles it is reached through, when ambiguous. */
          roles?: string[];
        }
      | {
          calendar?: string | null;
          grain: components['schemas']['SemanticGrain'];
          id: string;
          /** @enum {string} */
          kind: 'time_dimension';
          label?: string | null;
          name: string;
          roles?: string[];
          /** @description IANA zone overriding the governed one. */
          timezone?: string | null;
        };
    SemanticTarget:
      | {
          /** @enum {string} */
          kind: 'dimension';
          name: string;
          roles?: string[];
        }
      | {
          /** @enum {string} */
          kind: 'metric';
          selection: string;
        };
    /** @description A literal as written; binding types it after its target. */
    SemanticValue:
      | {
          /** @enum {string} */
          type: 'null';
        }
      | {
          /** @enum {string} */
          type: 'boolean';
          value: boolean;
        }
      | {
          /** @enum {string} */
          type: 'number';
          /** @description Exact decimal text (`"12.50"`), never a binary float. */
          value: string;
        }
      | {
          /** @enum {string} */
          type: 'text';
          value: string;
        }
      | {
          /** @enum {string} */
          type: 'date';
          /** @description ISO `YYYY-MM-DD`. */
          value: string;
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
      /** @description No se pudo crear la sesión, o metadatos no listos (`metadata_not_ready`) */
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
      /** @description PostgreSQL no disponible, o metadatos no listos (`metadata_not_ready`) */
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
      /** @description Sin permiso para revisar el catálogo del proyecto (`models_forbidden`); sin nada del catálogo */
      403: {
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
      /** @description Motor no disponible, lectura fallida, políticas o miembros ilegibles o inválidos (`policies_invalid`), auditoría no disponible (`audit_unavailable`) o metadatos no listos (`metadata_not_ready`) */
      503: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Tiempo agotado al revisar los permisos o al compilar */
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
  completeOidcLogin: {
    parameters: {
      query: {
        /** @description Código del proveedor */
        code: string;
        /** @description State de un solo uso */
        state: string;
      };
      header?: never;
      path?: never;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      /** @description Sesión de servidor creada; cookie HttpOnly SameSite=Strict */
      303: {
        headers: {
          [name: string]: unknown;
        };
        content?: never;
      };
      /** @description Intento o token inválido */
      401: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['Problem'];
        };
      };
      /** @description OIDC deshabilitado */
      404: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['Problem'];
        };
      };
      /** @description Admisión OIDC limitada */
      429: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['Problem'];
        };
      };
      /** @description Proveedor, PostgreSQL o capacidad de intentos no disponible, o metadatos no listos (`metadata_not_ready`) */
      503: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['Problem'];
        };
      };
    };
  };
  getOidcConfig: {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      /** @description Disponibilidad del login corporativo */
      200: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['LoginConfig'];
        };
      };
    };
  };
  startOidcLogin: {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      /** @description URL del proveedor y cookie temporal HttpOnly */
      200: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['LoginStart'];
        };
      };
      /** @description Parámetros no permitidos */
      400: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['Problem'];
        };
      };
      /** @description Origin inválido */
      403: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['Problem'];
        };
      };
      /** @description OIDC deshabilitado */
      404: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['Problem'];
        };
      };
      /** @description Admisión OIDC limitada */
      429: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['Problem'];
        };
      };
      /** @description Proveedor, PostgreSQL o capacidad de intentos no disponible, o metadatos no listos (`metadata_not_ready`) */
      503: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['Problem'];
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
      /** @description Origin ausente o distinto al configurado (`origin_forbidden`), o vía cerrada fuera del desarrollo sintético local (`legacy_query_disabled`) */
      403: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Consulta fuera del catálogo o de los límites (`invalid_query`), resultado mayor que el presupuesto de bytes (`result_too_large`) o memoria de consulta agotada (`query_memory_limit`) */
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
      /** @description Motor analítico no disponible, metadatos no listos (`metadata_not_ready`) o denegación que no pudo registrarse en la auditoría (`audit_unavailable`) */
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
  explain: {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    requestBody: {
      content: {
        'application/json': components['schemas']['ExplainRequest'];
      };
    };
    responses: {
      /** @description Decisión, plan certificado y recibo */
      200: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/json': components['schemas']['SemanticExplanation'];
        };
      };
      /** @description Sin sesión */
      401: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Un uso no permitido. Sin permiso de inspección del catálogo, solo si todo lo que la consulta nombra (y lo que unen los roles que escribe) es visible para la sesión con `view` o `view_masked`; si no, responde el 404. Lo que el plan lee por linaje se deniega sin nombre */
      403: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['SemanticDenial'];
        };
      };
      /** @description `model_not_found`: el modelo no existe en el proyecto, o la consulta no se puede explicar a esta sesión. Sin permiso de inspección del catálogo (`view` sobre `Model::"project:catalog"`), un archivo del proyecto ausente o inválido, cualquier fallo al enlazar la consulta con el modelo (nombres desconocidos o ambiguos incluidos) y un miembro o un rol hacia datos que la sesión no puede ver responden igual, con el detalle fijo «No se puede explicar la consulta solicitada.»: no confirma qué existe */
      404: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Petición con formato inválido; consulta incorrecta sobre miembros visibles; con permiso de inspección del catálogo, también modelo con errores y fallos al enlazar la consulta */
      422: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['SemanticRejection'];
        };
      };
      /** @description Demasiadas compilaciones simultáneas */
      429: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Políticas no válidas, motor o auditoría no disponibles, o metadatos no listos (`metadata_not_ready`) */
      503: {
        headers: {
          [name: string]: unknown;
        };
        content: {
          'application/problem+json': components['schemas']['Problem'];
        };
      };
      /** @description Tiempo agotado */
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
      /** @description PostgreSQL no disponible, o metadatos no listos (`metadata_not_ready`) */
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

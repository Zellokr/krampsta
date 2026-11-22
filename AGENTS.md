# Krampsta — guía para agentes

Krampsta es una app de escritorio nativa (Rust 2024 + Rune/rune-kit) para agentes de programación.
Lee `README.md` primero. Este archivo recoge las reglas para seguir desarrollándola.

## Mapa

- `crates/krampsta-core` — dominio puro. **Nunca** depende de Rune, tokio ni proveedores.
  `Session::apply` es la única forma de cambiar una sesión a partir de eventos.
- `crates/krampsta-agent` — `AgentEngine` (contrato), `RunContext`, `AgentHost` (tokio, canal +
  notificador, cambio de motor, listas de modelos en segundo plano), `EngineCatalog` (qué
  proveedores y modelos hay, cómo construir un motor), `MockAgent`, `MockCatalog`, `ToolServers`
  (cómo ve la UI los servidores MCP: estado por servidor y versión; `FixedServers` para escenas y
  tests). No depende de Rune.
- `crates/krampsta-providers` — motores reales: `sse` (parser), `http` (cliente `reqwest`, petición
  SSE cancelable, `HttpError`), `access` (URL + cabeceras por petición: clave fija o token de
  cuenta), `OpenAiEngine`, `responses` (motor de la API Responses), `list_models`, `copilot`
  (device flow, token de Copilot), `chatgpt` (código de dispositivo de OpenAI, refresh), `accounts`
  (iniciar/cerrar sesión), `secrets` (llavero del sistema) y `Catalog`. No depende de Rune.
- `crates/krampsta-tools` — las herramientas del agente sobre la carpeta del proyecto
  (`Workspace`, un `Toolbox`): `project` (ninguna ruta sale de la carpeta), `read_file`,
  `list_files`, `search` (crates de ripgrep: `ignore`, `grep-searcher`), `edit` (`edit_file`, que
  propone, y la escritura al aceptar), `command` (`run_command`), `policy` y `permissions` (reglas de permisos: comandos y herramientas),
  `snapshot` (copia desechable con las ediciones pendientes para `run_command` con `pending: true`;
  Cargo compila en la caché del usuario, `check-target`; no es un sandbox), `guidance` (al abrir
  el proyecto lee su `AGENTS.md` de la raíz, con tope `MAX_RULES`, y lista las skills de
  `.agents/skills/*/SKILL.md` y `.claude/skills/` sólo por nombre, descripción y ruta; el modelo
  abre la guía con `read_file` cuando la necesita).
  No depende de Rune.
- `crates/krampsta-mcp` — cliente MCP sobre el SDK oficial `rmcp`: `McpClient::connect`
  (`Transport::Stdio`, proceso con árbol que se mata entero y sin consola, que sólo hereda las
  variables básicas del sistema, `INHERITED_ENV`, más las suyas: nunca las claves de Krampsta;
  guarda el final de
  su stderr para explicar por qué no arrancó; `Transport::Http`, Streamable HTTP con cabeceras
  y token), `tools` y `call` (cancelable, respuesta en texto con tope `MAX_REPLY`). `fake` y el
  binario `krampsta-fake-mcp` son sólo para tests. `config` (`mcp.json`, formato `mcpServers` de
  Claude Desktop), `edits` (un servidor nuevo del formulario, validado, y las ediciones de
  `mcp.json`, que tocan sólo su entrada y conservan el resto), `McpServers` (conecta en segundo
  plano en su propio runtime tokio, aplica al momento lo que cambia Ajustes y escribe el archivo
  en orden, en una tarea propia; cada intento de conexión lleva un número y uno viejo se descarta; las
  herramientas de un servidor listo entran en las ejecuciones siguientes como
  `mcp__<servidor>__<herramienta>`) y `ToolSet` (el `Toolbox` que recibe el agente: el del
  proyecto más los servidores; una herramienta MCP sigue las reglas `tools` y, sin regla, pregunta).
  La app envuelve con
  `ToolSet` cada proyecto que abre. No depende
  de Rune; sólo de `krampsta-agent`.
- `crates/krampsta-store` — historial en disco: `FileStore` (implementa `SessionStore` de
  `krampsta-agent`; un JSON por sesión, escrito por un hilo propio, de forma atómica; las imágenes
  de los mensajes, en `images/<sesión>-<mensaje>.png`, escritas una vez), `load`
  (lectura bloqueante, fuera del hilo de la UI) y `format` (la forma en disco, versionada,
  separada del dominio). No depende de Rune.
- `crates/krampsta-ui` — `KrampstaApp` (estado), `picker.rs` (selector de proveedor y modelo), `app.rs`
  (spec, vista raíz, poll, teclas, escenas), `views/` (pantallas y paneles), `components/`
  (piezas), `accounts.rs` (diálogo de inicio de sesión), `approvals.rs` (permitir comandos),
  `projects.rs` (carpeta del proyecto: diálogo nativo con `tx.open_folder`, se abre con el
  `ProjectOpener` en el host y sólo entonces cambian las herramientas; `groups` agrupa las
  sesiones por `Session::project`, y una sesión sin proyecto adopta el abierto al enviar),
  `prefs.rs` (ajustes entre arranques: tema, anchos, proveedor y modelo, proyecto, grupos
  plegados; `poll` compara sin asignar y guarda medio segundo después de que paren de cambiar),
  `components/inline_diff.rs` (el cambio de un `edit_file` dentro de su tarjeta: diff compacto
  de sólo lectura; `ChangeProposed` lleva el `call` y `pump` guarda su `FileDiff` en `card_diffs`),
  `panes.rs` (dos sesiones lado a lado: `split` guarda la del otro panel; la seleccionada es
  siempre la del panel activo; claves del panel derecho por ruta, `krampsta-pane-1/…`),
  `tabs.rs` (sesiones abiertas en pestañas sobre el chat, `TabStrip` de rune-kit; la activa es
  la seleccionada; la última no se cierra; Ctrl+W), `costs.rs` (coste por sesión: tokens por modelo, `Session::by_model`, por los precios por
  millón que pone el usuario; sólo proveedores `Cloud`, sin precios de serie),
  `servers.rs` (los servidores MCP en Ajustes, desde `ToolServers`: la lista se copia en `pump`
  sólo cuando cambia su versión; un servidor que falla lo avisa en el composer; el formulario
  para añadir y la confirmación de quitar; la vista en `views/mcp.rs`),
  `images.rs` (imágenes pegadas y de los mensajes: se decodifican en un hilo propio y llegan por
  `pump` a `SessionView`; nunca en `view`; sólo se envían si `Provider::images`),
  `files.rs` (el árbol de archivos de la vista «Files»: la lista sale de `Toolbox::files` por
  `AgentHost::list_files`, numerada, y llega por `pump`; caduca al cambiar de proyecto o escribir un
  cambio; el panel con su selector es `views/inspector.rs`),
  `changes.rs` (revisar los cambios
  propuestos: aceptar o rechazar archivos y bloques), `theme.rs` (único sitio de colores),
  `keys.rs` (claves estables), `sample.rs` (ejemplos sólo para capturas y tests:
  `with_sample_sessions`; la app real arranca con una sesión vacía y el historial).
- `crates/krampsta-app` — binario `krampsta`; construye el catálogo, abre el proyecto inicial
  (`krampsta <carpeta>`, el último abierto o la carpeta actual; en headless, el proyecto y
  las sesiones de ejemplo) y arranca.
- Dependencias permitidas: `app → ui → agent → core`, `ui → core`, `app → providers → agent`,
  `providers → core`, `app → tools → agent`, `app → store → agent`, `store → core`, `app → mcp → agent`. Sin ciclos;
  la UI nunca depende de `krampsta-providers`, `krampsta-tools`, `krampsta-store` ni `krampsta-mcp`.
- Historial: se lee una vez, en `setup` (`load_history`), y llega por `poll` (`take_history`).
  La UI guarda con `SessionStore` (sólo encola), en momentos discretos (enviar, fin de
  herramienta, fin de ejecución, borrar), nunca por delta ni por frame. Con almacén, los ids de
  sesión salen de `IdGen::from_clock` para no repetir uno guardado. Las ejecuciones headless
  (`rune::is_headless()`) no abren almacén. Un cambio en la forma en disco sube
  `format::VERSION`; los tests usan `MemoryStore` o `tempfile`, nunca la carpeta real.

## Comandos (todos deben pasar antes de terminar)

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
bash scripts/snapshots.sh   # y mira las PNG: compilar no prueba que la UI esté bien
```

## Rune: reglas que importan

- La API de Rune evoluciona: **no inventes métodos**. Consulta el código en el commit fijado
  (`~/.cargo/git/checkouts/rune-*/47eaa86/`), sobre todo `crates/rune/README.md`,
  `crates/rune-kit/README.md` y los ejemplos (`crates/rune-kit/examples/shell.rs`, `diff.rs`).
- Antes de hacer un componente propio, busca uno en `rune-kit` (Sidebar, SplitView, TextArea,
  DiffView, Markdown, Tree, Table, Menu, Dialog, Toast…).
- Las vistas son funciones puras de `KrampstaApp`: nada de I/O, bloqueos, `std::fs` ni trabajo
  costoso en `view`. El trabajo lento va al `AgentHost` (o a un worker / `rune::Latest`) y vuelve
  por `poll`. `poll` devuelve `true` sólo si cambió algo visible.
- Claves: siempre con prefijo `krampsta-` (rune-kit usa claves cortas como `input`, `scroll`).
  Añade las nuevas a `keys.rs` y úsalas en tests y scripts.
- Gotchas de layout (flexbox): un `.scroll(..)` dentro de `.grow()` necesita `.min_h(0.0)` en
  cada columna intermedia; `.grow` tiene base 0.
- El texto en streaming no se pinta según llega: `pace.rs` (`Pacer`) estima a qué ritmo llega y
  lo suelta a ese ritmo desde `poll`, con una reserva de ~0,25 s que cubre los huecos entre
  ráfagas (`tx.wake_after(Duration::ZERO)` mientras queda texto), por palabras enteras, con
  `Markdown::reveal` para el fundido y el chat como growth stack con `edge::WORD_FADE` (los bordes
  `FEATHER` recortan la fila a su altura animada y el texto salía línea a línea). Una
  escena que prepara una respuesta a medias llama a `catch_up` para mostrarla. Para ver cómo sale,
  una tira de fotogramas: `krampsta --scene streaming --frames 16 --start 0.05 --out DIR --strip
  tira.png --crop 280,140,470,200 --every 1 --cols 2`; una captura suelta no enseña animaciones.
  `KRAMPSTA_TRACE_PACE=1` registra métricas de ritmo por fotograma en stderr, sin contenido.
- Lo que la ventana guarda por sesión (borrador, ritmo del streaming) va en `SessionView`
  (`session_view.rs`), indexado por `SessionId`: nunca un campo suelto en `KrampstaApp`.
- Estado con modos = `enum` cuyas variantes llevan sólo sus datos (ver `AgentStatus`), no
  grupos de booleanos.
- Parches: `[patch.crates-io]` del `Cargo.toml` raíz replica los de Rune, todos al mismo SHA que
  `rune`/`rune-kit`. Para actualizar, sigue «Actualizar Rune» del README.

## Motor de agente

- Un motor nuevo implementa `AgentEngine::run(self: Arc<Self>, RunRequest, RunContext)`:
  emite `ctx.text(..)`, `ctx.tool_started(..)`, `ctx.tool_finished(..)` y, una vez por petición
  al proveedor, `ctx.usage(..)` con los tokens que contó (también si falla tras contarlos); espera con
  `ctx.sleep(..)` o futuros cancelables; comprueba `ctx.check_cancelled()` entre pasos.
- Herramientas: el motor manda `ctx.tool_specs()` al proveedor y ejecuta cada llamada con
  `ctx.run_tool(..)` (hilo bloqueante, cancelable, emite la tarjeta); las de un paso van a la vez
  con `steps::run_calls`. El bucle (pedir → herramientas → resultados → pedir) vive en cada motor,
  porque el formato es de cada API, con el tope `steps::MAX_STEPS`. Los turnos anteriores se
  mandan sólo como texto; el tráfico de herramientas vive en memoria durante la ejecución.
- Instrucciones: todo motor las forma con `steps::instructions(&ctx)` (las de Krampsta, la pista
  de herramientas y, con herramientas, `ctx.guidance()`: lo que dice el proyecto abierto, no las
  reglas de Krampsta). Es sólo texto: no concede permisos. Un `Toolbox` que envuelve a otro
  (p. ej. para sumar herramientas MCP) reenvía su `guidance`.
- Las herramientas nunca escriben: `edit_file` devuelve un `FileChange` (antes/después) que la
  UI muestra para revisar, y guarda la propuesta como pendiente (las lecturas y ediciones
  siguientes del agente la ven). Sólo `Toolbox::write`, al aceptar desde el panel, toca el
  disco: comprueba que el archivo sigue como se propuso y escribe de forma atómica. Aceptar o
  rechazar un bloque es `splice_lines` (lineal) sobre los dos textos.
- Permisos: antes de ejecutar, `Toolbox::check` dice `Allow`, `Ask` o `Deny`. Con `Ask`, el
  contexto emite `ApprovalRequested` y espera (cancelable) a que la UI llame a
  `AgentHost::answer`; «siempre» guarda la regla con `Toolbox::remember`. Las reglas viven en
  `permissions.json` (carpeta de configuración del usuario); `deny` gana y mira cada parte de un
  comando encadenado; una regla `*` de `allow` sólo cubre comandos simples. Cada comando deja una
  línea en `commands.log` (qué, dónde, quién lo permitió, cómo acabó). Las herramientas que no
  son comandos ni del `Workspace` (las de un servidor MCP) siguen `tools` (`allow`/`deny` por
  nombre o prefijo `*`): las aplica el `Workspace` o, sin carpeta, `Permissions` (un `Toolbox`
  sin herramientas). Para un nombre `mcp__…`, la `ToolSet` toma un `NotNeeded` como `Ask`: un
  toolbox que no conoce la herramienta nunca la deja pasar sin preguntar. Si `permissions.json`
  no se puede leer, todo pregunta y nada se guarda encima.
- `run_command` corre como árbol de procesos que se mata entero (Job Object / grupo de procesos,
  con `process-wrap`) al vencer el tiempo, al cancelar o al salir su shell. La salida va a la UI
  por lotes (`Call::output`, cada 50 ms o 16 KB) y al modelo como cabeza + cola acotadas.
- Una herramienta nueva va en `krampsta-tools`: lee sólo lo necesario (streaming, sin cargar
  archivos enteros), pone tope a su salida, comprueba la cancelación en los bucles largos y
  resuelve rutas con `Project::resolve`. Se prueba con `tempfile`, nunca con el disco real.
- **No** emitas `Started` ni eventos terminales: los pone el host. Devuelve `Ok(())`,
  `Err(AgentError::..)` o `Err(AgentError::Cancelled)`.
- Nunca bloquees un hilo de tokio (nada de I/O síncrona en el motor).
- Los motores de proveedor viven en `krampsta-providers`. Un proveedor nuevo se añade a `Catalog`
  (`catalog.rs`): sale solo en los selectores de la UI, que sólo conoce `EngineCatalog`.
- El usuario elige proveedor y modelo en la app; cambiar de motor afecta a las ejecuciones
  nuevas, no a la que está en marcha. Las listas de modelos se piden con `AgentHost::fetch_models`
  y llegan por `poll`, nunca desde `view`.
- Las capturas fijan `KRAMPSTA_PROVIDER=mock` (`scripts/snapshots.sh`): no deben depender de las
  claves de la máquina. Las escenas que muestran proveedores los preparan a mano.
- Peticiones HTTP siempre con `http::client()` y `http::stream_sse` (cancelación y errores
  legibles ya resueltos). Cada motor se prueba contra el servidor falso de
  `crates/krampsta-providers/tests/` con un stream grabado real, no sólo con JSON inventado.
- Las claves nunca se imprimen: `Debug` a mano en las configs que las llevan; cabeceras con
  `access::bearer` (marcadas como sensibles).
- Cuentas (ChatGPT, Copilot): lo que dependa de APIs no públicas va en su propio módulo
  (`chatgpt.rs`, `copilot.rs`). Un refresh token rotado se guarda en el llavero al momento.
  En el llavero sólo secretos pequeños y duraderos (Windows limita cada entrada a 2560 bytes);
  los tokens de corta duración se renuevan en memoria. Las llamadas al llavero desde async van
  por `spawn_blocking`. Los tests usan `MemoryStore` y un servidor falso, nunca el llavero real.
- Claves de API guardadas desde la app: `EngineCatalog::set_api_key` (llavero), y el proveedor lo
  anuncia con `Provider::api_key` (`KeySource`). La guardada manda sobre la variable de entorno.
- Para ver qué manda de verdad un backend, `KRAMPSTA_TRACE_SSE=1` escribe en stderr el tipo de
  cada evento del motor Responses (y los nombres de campo de los que ignora), nunca su contenido.
  Pide al usuario esa traza antes de cambiar la petición a ciegas.
- Para grabar fixtures, `KRAMPSTA_RECORD_SSE=<carpeta>` guarda cada stream tal cual (un `.sse`
  por petición, todos los motores). Lleva la conversación entera: el usuario lo activa, y la
  grabación se revisa (y se recorta lo personal o cifrado que no haga falta) antes de versionarla
  en `crates/krampsta-providers/tests/fixtures/`.
- La petición a ChatGPT imita a Codex CLI (`github.com/openai/codex`, `core/src/client.rs`): la
  forma de cada modelo sale del catálogo `codex/models` (`use_responses_lite`,
  `default_reasoning_level`, `supports_reasoning_summary_parameter`). Un modelo Lite exige su
  cabecera y `parallel_tool_calls: false` (si no, HTTP 400). Comprobado con trazas: `gpt-6.2-sol`
  es Lite, con esfuerzo `low` por defecto, y no emite elementos de razonamiento con ningún
  esfuerzo (probados `low` a `max`; `ultra` es un modo de Codex que la API rechaza):
  el backend no lo expone para ese modelo. No lo persigas cambiando la petición.

## Código

- Rust 2024, `unsafe` prohibido a nivel de workspace (`unsafe_code = "forbid"`).
- `Result` para lo que pueda fallar; sin `unwrap`/`expect` fuera de tests (clippy avisa).
- Errores con `thiserror`; mensajes que un usuario pueda leer.
- Tests unitarios junto al código (`#[cfg(test)] mod tests` al final del archivo); tests de
  interacción en `crates/krampsta-ui/tests/` con `rune::Runtime` (ver `tests/ui.rs`: `click`,
  `type_text` con `Ime::Commit`, `press`, `pump_until_finished`).
- Los helpers de `tests/*.rs` (fuera de funciones `#[test]`) no están cubiertos por
  `allow-unwrap-in-tests`: esos archivos empiezan con `#![allow(clippy::unwrap_used, clippy::expect_used)]`.
- Una escena headless nueva (`SCENES` en `app.rs`) por cada estado visual nuevo importante.
  Las escenas **asignan** el estado que muestran (p. ej. la lista de proveedores entera), no
  añaden a lo que haya: `krampsta --snap` arranca con el catálogo real del entorno.
- El runtime del `AgentHost` usa `enable_all()`: HTTP necesita el driver de I/O, no sólo el de tiempo.
- Archivos de menos de ~800 líneas; si crecen, divide por capacidad.
- Sin secretos en el repositorio (claves de API por entorno o llavero del sistema).
- Commits pequeños y descriptivos; `Cargo.lock` siempre versionado.

## Rust idiomático y rendimiento

- Presta por defecto (`&T`, `&str`, `&[T]` en parámetros); clona sólo si hace falta la propiedad.
  `Arc`/`Rc` sólo cuando el modelo de propiedad lo exige. `Copy` pequeños, por valor.
- Nada de clones ni asignaciones grandes en `view` ni en `poll` (se llaman en cada frame): presta
  del estado de `KrampstaApp`; si un dato se comparte mucho, `Arc<str>` / `Arc<[T]>`.
- Evita asignar de más: `_or_else` en vez de `_or` con valores costosos, `with_capacity` cuando
  se sabe el tamaño, reutiliza buffers en bucles (p. ej. el parser SSE).
- Despacho estático (`impl Trait`, genéricos) por defecto; `dyn` para polimorfismo en tiempo de
  ejecución (motores, catálogo). Retrasa el `Box` hasta una frontera de diseño.
- Async: ningún `Mutex` ni guarda de `RefCell` vivos a través de un `.await`; trabajo síncrono o
  de CPU a `spawn_blocking`; canales acotados salvo razón escrita; nada de `block_on` en tokio.
- Errores: `?` y `let … else`; nunca tragues un `Err` en silencio (propágalo o muéstralo).
  Un `expect` en tests explica el invariante.
- Lints: no añadas `#[allow]` nuevos (salvo la cabecera de `tests/*.rs`); si hace falta,
  `#[expect(lint, reason = "…")]` local.
- Optimiza sólo con medidas: `criterion` para tiempos, `dhat` para memoria, flamegraph para
  perfiles. Una mejora de rendimiento lleva su número antes/después en el commit.

## Hoja de ruta

La lista de tareas del proyecto, con casillas, es un documento de Claude Docs (no está en el
repo): https://claude.ai/artifact/Dp946THS8hdANaaJg2uxiV

- Al empezar una tarea, léelo con las herramientas de Claude Docs (no con fetch: es privado) para
  saber en qué hito estamos.
- Al terminar una tarea según los criterios del propio documento (validación + `verify-ui` si
  cambia la UI), marca su casilla (`set` `checked: true` en su listItem). No marques lo que no
  esté hecho y comprobado.
- Una tarea nueva que surja va al hito que corresponda, en una línea y con un verbo. Antes de
  añadirla, relee las tareas de **todos** los hitos: puede que otra ya la cubra.
- Si la sesión no tiene acceso a Claude Docs, dilo al usuario en vez de suponer el estado.

## Skills

- `verify-ui` (`.claude/skills/verify-ui/`): el ciclo de capturas headless y la lista de
  revisión. Úsala tras cualquier cambio visible.
- `rune-api` (skill de usuario, `~/.claude/skills/rune-api/`): cómo consultar la API real de
  Rune en el commit fijado antes de usarla.

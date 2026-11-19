# Krampsta — guía para agentes

Krampsta es una app de escritorio nativa (Rust 2024 + Rune/rune-kit) para agentes de programación.
Lee `README.md` primero. Este archivo recoge las reglas para seguir desarrollándola.

## Mapa

- `crates/krampsta-core` — dominio puro. **Nunca** depende de Rune, tokio ni proveedores.
  `Session::apply` es la única forma de cambiar una sesión a partir de eventos.
- `crates/krampsta-agent` — `AgentEngine` (contrato), `RunContext`, `AgentHost` (tokio, canal +
  notificador, cambio de motor, listas de modelos en segundo plano), `EngineCatalog` (qué
  proveedores y modelos hay, cómo construir un motor), `MockAgent`, `MockCatalog`. No depende de Rune.
- `crates/krampsta-providers` — motores reales: `sse` (parser), `http` (cliente `reqwest`, petición
  SSE cancelable, `HttpError`), `access` (URL + cabeceras por petición: clave fija o token de
  cuenta), `OpenAiEngine`, `responses` (motor de la API Responses), `list_models`, `copilot`
  (device flow, token de Copilot), `chatgpt` (código de dispositivo de OpenAI, refresh), `accounts`
  (iniciar/cerrar sesión), `secrets` (llavero del sistema) y `Catalog`. No depende de Rune.
- `crates/krampsta-tools` — las herramientas del agente sobre la carpeta del proyecto
  (`Workspace`, un `Toolbox`): `project` (ninguna ruta sale de la carpeta), `read_file`,
  `list_files`, `search` (crates de ripgrep: `ignore`, `grep-searcher`), `edit` (`edit_file`, que
  propone, y la escritura al aceptar). No depende de Rune.
- `crates/krampsta-ui` — `KrampstaApp` (estado), `picker.rs` (selector de proveedor y modelo), `app.rs`
  (spec, vista raíz, poll, teclas, escenas), `views/` (pantallas y paneles), `components/`
  (piezas), `accounts.rs` (diálogo de inicio de sesión), `changes.rs` (revisar los cambios
  propuestos: aceptar o rechazar archivos y bloques), `theme.rs` (único sitio de colores),
  `keys.rs` (claves estables), `sample.rs` (datos
  de ejemplo).
- `crates/krampsta-app` — binario `krampsta`; sólo construye el catálogo y arranca.
- Dependencias permitidas: `app → ui → agent → core`, `ui → core`, `app → providers → agent`,
  `providers → core`, `app → tools → agent`. Sin ciclos; la UI nunca depende de
  `krampsta-providers` ni de `krampsta-tools`.

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
- Las herramientas nunca escriben: `edit_file` devuelve un `FileChange` (antes/después) que la
  UI muestra para revisar, y guarda la propuesta como pendiente (las lecturas y ediciones
  siguientes del agente la ven). Sólo `Toolbox::write`, al aceptar desde el panel, toca el
  disco: comprueba que el archivo sigue como se propuso y escribe de forma atómica. Aceptar o
  rechazar un bloque es `splice_lines` (lineal) sobre los dos textos.
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

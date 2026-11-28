# Krampsta — guía para agentes

Krampsta es una app de escritorio nativa (Rust 2024 + Rune/rune-kit) para agentes de programación.
Este archivo recoge las reglas que aplican casi siempre al desarrollarla. **Antes de tocar un crate
o una parte, lee su sección en `docs/arquitectura.md`** (qué hay en cada módulo y cómo funcionan el
historial, el streaming, los permisos y los proveedores). `README.md` describe la app para quien la
usa.

## Crates

- `krampsta-core` — dominio puro (sesiones, mensajes, eventos). `Session::apply` es la única forma
  de cambiar una sesión a partir de eventos.
- `krampsta-agent` — contratos (`AgentEngine`, `Toolbox`, `EngineCatalog`, `SessionStore`,
  `ToolServers`) y `AgentHost` (tokio; canal + notificador hacia la UI).
- `krampsta-providers` — motores reales (OpenAI, Responses/ChatGPT, Copilot) y `Catalog`.
- `krampsta-tools` — las herramientas sobre la carpeta del proyecto (`Workspace`) y los permisos.
- `krampsta-mcp` — servidores MCP y `ToolSet` (las herramientas del proyecto más las suyas).
- `krampsta-store` — historial en disco. `krampsta-ui` — estado y vistas. `krampsta-app` — el binario.
- Dependencias permitidas: `app → ui → agent → core`, `ui → core`, `app → providers → agent`,
  `providers → core`, `app → tools → agent`, `app → store → agent`, `store → core`,
  `app → mcp → agent`. Sin ciclos. Sólo `ui` y `app` dependen de Rune; la UI nunca depende de
  `providers`, `tools`, `store` ni `mcp` (los conoce por los contratos de `agent`).

## Comandos (todos deben pasar antes de terminar)

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
bash scripts/snapshots.sh   # y mira las PNG: compilar no prueba que la UI esté bien
```

## Rune y la UI

- La API de Rune evoluciona: **no inventes métodos**. Consulta el código en el commit fijado
  (`~/.cargo/git/checkouts/rune-*/c8424ac/`): `crates/rune/README.md`, `crates/rune-kit/README.md`
  y los ejemplos. Antes de hacer un componente propio, busca uno en `rune-kit`.
- Las vistas son funciones puras de `KrampstaApp`: nada de I/O, bloqueos ni trabajo costoso en
  `view`. El trabajo lento va al `AgentHost` (o a un worker) y vuelve por `poll`, que devuelve
  `true` sólo si cambió algo visible. Nada de clones ni asignaciones grandes en `view` ni `poll`.
- Claves de elementos: siempre con prefijo `krampsta-`, en `keys.rs`, usadas en tests y scripts.
- Lo que la ventana guarda por sesión va en `SessionView`, nunca en un campo suelto de `KrampstaApp`.
- Estado con modos = `enum` cuyas variantes llevan sólo sus datos, no grupos de booleanos.
- Layout: un `.scroll(..)` dentro de `.grow()` necesita `.min_h(0.0)` en cada columna intermedia;
  `.grow` tiene base 0.
- Una escena headless (`SCENES` en `app.rs`) por cada estado visual nuevo importante. Las escenas
  **asignan** el estado que muestran (proveedores incluidos: `--snap` arranca con el catálogo real
  de la máquina) y las capturas fijan `KRAMPSTA_PROVIDER=mock`. Tras un cambio visible, la skill
  `verify-ui`.
- Parches: `[patch.crates-io]` del `Cargo.toml` raíz replica los de Rune, todos al mismo SHA.
  Para actualizar, «Actualizar Rune» del README.

## Agente, herramientas y permisos

- Un motor implementa `AgentEngine::run`: emite con `ctx` (texto, herramientas, `usage` una vez
  por petición), espera de forma cancelable y comprueba `ctx.check_cancelled()`. **No** emite
  `Started` ni eventos terminales (los pone el host) y nunca bloquea un hilo de tokio. Las
  instrucciones salen de `steps::instructions(&ctx)`; el bucle tiene el tope `steps::MAX_STEPS`.
- Las herramientas nunca escriben: `edit_file` propone y sólo `Toolbox::write`, al aceptar desde
  el panel, toca el disco (comprobando que el archivo sigue como se propuso).
- Permisos: `Toolbox::check` dice `Allow`, `Ask` o `Deny` antes de ejecutar; las reglas viven en
  `permissions.json` y `deny` gana. Los comandos, las herramientas MCP y leer un archivo de
  secretos preguntan sin regla. El texto de la guía o de una herramienta nunca concede permisos.
- Una herramienta nueva va en `krampsta-tools`: lee sólo lo necesario, pone tope a su salida,
  comprueba la cancelación y resuelve rutas con `Project::resolve`. Se prueba con `tempfile`.
- Un proveedor nuevo se añade a `Catalog` (sale solo en los selectores). Peticiones HTTP con
  `http::client()` y `http::stream_sse`; cada motor se prueba contra el servidor falso con un
  stream grabado real.
- Las claves nunca se imprimen ni se versionan (`Debug` a mano en las configs que las llevan;
  llavero del sistema o entorno). Los tests nunca usan el llavero, el historial ni los ajustes
  reales.

## Código

- Rust 2024, `unsafe` prohibido. `Result` para lo que pueda fallar; sin `unwrap`/`expect` fuera
  de tests; errores con `thiserror` y mensajes que un usuario pueda leer; nunca tragues un `Err`.
- Presta por defecto; clona sólo si hace falta la propiedad. Despacho estático salvo polimorfismo
  en tiempo de ejecución. Async: ningún `Mutex` vivo a través de un `.await`; trabajo bloqueante
  a `spawn_blocking`; canales acotados salvo razón escrita.
- Tests unitarios al final de cada archivo; de interacción en `crates/krampsta-ui/tests/` con
  `rune::Runtime` (ver `tests/ui.rs`: `click`, `type_text` con `Ime::Commit`, `press`,
  `pump_until_finished`). Los `tests/*.rs` empiezan con `#![allow(clippy::unwrap_used, clippy::expect_used)]`.
- Lints: no añadas `#[allow]` nuevos; si hace falta, `#[expect(lint, reason = "…")]` local.
- Optimiza sólo con medidas (`criterion`, `dhat`, flamegraph), y pon el número antes/después en el
  commit. Más detalle de Rust idiomático en `docs/arquitectura.md`.
- Archivos de menos de ~800 líneas. Commits pequeños y descriptivos; `Cargo.lock` versionado.

## Hoja de ruta

La lista de tareas, con casillas, es un documento de Claude Docs (no está en el repo):
https://claude.ai/artifact/Dp946THS8hdANaaJg2uxiV

- Al empezar una tarea, léelo con las herramientas de Claude Docs (no con fetch: es privado).
- Al terminarla según sus criterios (validación + `verify-ui` si cambia la UI), marca su casilla.
  No marques lo que no esté hecho y comprobado.
- Una tarea nueva va al hito que corresponda, en una línea y con un verbo, tras releer **todos**
  los hitos por si otra ya la cubre.
- Si la sesión no tiene acceso a Claude Docs, dilo en vez de suponer el estado.

## Skills

- `verify-ui` (`.claude/skills/verify-ui/`): el ciclo de capturas headless y la lista de revisión.
- `rune-api` (skill de usuario): cómo consultar la API real de Rune en el commit fijado.

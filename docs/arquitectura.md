# Krampsta — arquitectura

La referencia detallada para quien desarrolla Krampsta: qué hay en cada crate y módulo, y cómo
funcionan las partes con más detalle. Las reglas que aplican casi siempre están en `AGENTS.md`;
lee aquí la sección de lo que vayas a tocar.

## Módulos por crate

- `crates/krampsta-core` — dominio puro. **Nunca** depende de Rune, tokio ni proveedores.
  `Session::apply` es la única forma de cambiar una sesión a partir de eventos. `checkpoint`
  (`FileWrite`: lo que Krampsta escribió para una sesión, por el turno que lo propuso;
  `Session::rewind` quita la conversación desde un mensaje del usuario y devuelve un `Restore` por
  archivo, con lo que tenía antes de la primera escritura y lo último que se escribió).
- `crates/krampsta-agent` — `AgentEngine` (contrato), `RunContext`, `AgentHost` (tokio, canal +
  notificador, cambio de motor, listas de modelos, archivos y diff del repo en segundo plano),
  `EngineCatalog` (qué proveedores y modelos hay, cómo construir un motor), `MockAgent`,
  `MockCatalog`, `Toolbox` (las herramientas como las ve un motor: `specs`, `check`, `run`,
  `files`, `repo_diff`, `restore`…; `AgentHost::restore_files` devuelve los archivos de un rewind
  en orden, en un hilo bloqueante, y responde con un `Restored`), `ToolServers` (cómo ve la UI los servidores MCP: estado por servidor y
  versión; `FixedServers` para escenas y tests), `replay` (qué vuelve al modelo de los turnos
  anteriores: sus herramientas recortadas por antigüedad, y el resumen en lugar de lo que cubre;
  `HISTORY_BUDGET`), `compact` (resumir la conversación antes de una ejecución) y `plan`
  (`update_plan`: el plan del modelo para una tarea de varios pasos; no guarda nada, cada llamada
  lleva el plan entero y la tarjeta lo dibuja desde sus argumentos con `Plan::parse`). No depende de Rune.
- `crates/krampsta-providers` — motores reales: `sse` (parser), `http` (cliente `reqwest`, petición
  SSE cancelable, `HttpError`), `access` (URL + cabeceras por petición: clave fija o token de
  cuenta), `OpenAiEngine`, `responses` (motor de la API Responses), `anthropic` (`AnthropicEngine`, la API
  Messages de Claude: `thinking` adaptativo con resumen, `output_config.effort`, `cache_control`,
  `fallbacks` en los modelos que lo admiten; los bloques del asistente vuelven tal cual dentro de la
  ejecución, firma del razonamiento incluida), `steps` (lo común del bucle:
  instrucciones, llamadas a la vez, recorte del historial), `list_models`, `copilot` (device flow,
  token de Copilot), `chatgpt` (código de dispositivo de OpenAI, refresh), `accounts`
  (iniciar/cerrar sesión), `secrets` (llavero del sistema) y `Catalog`. No depende de Rune.
- `crates/krampsta-tools` — las herramientas del agente sobre la carpeta del proyecto
  (`Workspace`, un `Toolbox`): `project` (ninguna ruta sale de la carpeta), `read_file`,
  `list_files`, `update_plan` (de `krampsta_agent::plan`, sin permiso), `fetch` (`fetch_url`: una
  página http/https con el cliente bloqueante de `reqwest`, HTML a texto con `html2text`, topes de
  descarga y de salida, redirecciones sólo dentro del mismo sitio), `search` (crates de ripgrep: `ignore`, `grep-searcher`), `edit` (`edit_file`, que
  propone, y la escritura al aceptar), `command` (`run_command`), `policy` y `permissions` (reglas
  de permisos: comandos y herramientas), `secrets` (qué archivos guardan secretos, por su nombre),
  `snapshot` (copia desechable con las ediciones pendientes para `run_command` con `pending: true`;
  Cargo compila en la caché del usuario, `check-target`; no es un sandbox), `git` (el diff frente a
  `HEAD` para la vista «Git»: ejecuta `git`, sin consola, con topes), `guidance` (al abrir el
  proyecto lee su `AGENTS.md` de la raíz, con tope `MAX_RULES`, y lista las skills de
  `.agents/skills/*/SKILL.md` y `.claude/skills/` sólo por nombre, descripción y ruta; el modelo
  abre la guía con `read_file` cuando la necesita). No depende de Rune.
- `crates/krampsta-mcp` — cliente MCP sobre el SDK oficial `rmcp`: `McpClient::connect`
  (`Transport::Stdio`, proceso con árbol que se mata entero y sin consola, que sólo hereda las
  variables básicas del sistema, `INHERITED_ENV`, más las suyas, nunca las claves de Krampsta, y
  guarda el final de su stderr para explicar por qué no arrancó; `Transport::Http`, Streamable
  HTTP con cabeceras y token), `tools` y `call` (cancelable, respuesta en texto con tope
  `MAX_REPLY`). `fake` y el binario `krampsta-fake-mcp` son sólo para tests. `config` (`mcp.json`,
  formato `mcpServers` de Claude Desktop), `edits` (un servidor nuevo del formulario, validado, y
  las ediciones de `mcp.json`, que tocan sólo su entrada y conservan el resto), `McpServers`
  (conecta en segundo plano en su propio runtime tokio, aplica al momento lo que cambia Ajustes y
  escribe el archivo en orden, en una tarea propia; cada intento de conexión lleva un número y uno
  viejo se descarta; las herramientas de un servidor listo entran en las ejecuciones siguientes
  como `mcp__<servidor>__<herramienta>`) y `ToolSet` (el `Toolbox` que recibe el agente: el del
  proyecto más los servidores; una herramienta MCP sigue las reglas `tools` y, sin regla,
  pregunta). La app envuelve con `ToolSet` cada proyecto que abre. No depende de Rune; sólo de
  `krampsta-agent`.
- `crates/krampsta-store` — historial en disco: `FileStore` (implementa `SessionStore` de
  `krampsta-agent`; un JSON por sesión, escrito por un hilo propio, de forma atómica; las imágenes
  de los mensajes, en `images/<sesión>-<mensaje>.png`, escritas una vez), `load` (lectura
  bloqueante, fuera del hilo de la UI) y `format` (la forma en disco, versionada, separada del
  dominio; versión 2: el rol `summary`; `writes`, opcional, guarda los checkpoints). No depende de Rune.
- `crates/krampsta-ui` — `KrampstaApp` (estado, `state.rs`), `app.rs` (spec, vista raíz, poll,
  teclas, escenas), `views/` (pantallas y paneles), `components/` (piezas) y, por capacidad:
  - `picker.rs` (selector de proveedor y modelo), `accounts.rs` (diálogo de inicio de sesión),
    `approvals.rs` (permitir comandos y herramientas).
  - `projects.rs` (carpeta del proyecto: diálogo nativo con `tx.open_folder`, se abre con el
    `ProjectOpener` en el host y sólo entonces cambian las herramientas; `groups` agrupa las
    sesiones por `Session::project`, y una sesión sin proyecto adopta el abierto al enviar).
  - `prefs.rs` (ajustes entre arranques: tema, anchos, proveedor y modelo, proyecto, grupos
    plegados; `poll` compara sin asignar y guarda medio segundo después de que paren de cambiar).
  - `session_view.rs` (lo que la ventana guarda por sesión: borrador, ritmo del streaming,
    imágenes), `pace.rs` (el ritmo del streaming), `images.rs` (imágenes pegadas y de los
    mensajes: se decodifican en un hilo propio y llegan por `pump` a `SessionView`; nunca en
    `view`; sólo se envían si `Provider::images`).
  - `tabs.rs` (sesiones en pestañas, `TabStrip` de rune-kit; la activa es la seleccionada; la
    última no se cierra; Ctrl+W), `panes.rs` (dos sesiones lado a lado: `split` guarda la del otro
    panel; la seleccionada es siempre la del panel activo; claves del panel derecho por ruta,
    `krampsta-pane-1/…`).
  - `changes.rs` (revisar los cambios propuestos: aceptar o rechazar archivos y bloques),
    `files.rs` (la vista «Files»: la lista sale de `Toolbox::files` por `AgentHost::list_files`,
    numerada, y llega por `pump`; caduca al cambiar de proyecto o escribir un cambio,
    `disk_changed`), `git.rs` (la vista «Git»: el diff de `Toolbox::repo_diff` por
    `AgentHost::diff_repo`; caduca con la lista de archivos); el panel con su selector es
    `views/inspector.rs`.
  - `components/summary.rs` (el resumen de mensajes anteriores en el chat: una línea con su
    cabecera, plegada; se abre como el razonamiento, con `open_reasoning`).
  - `rewind.rs` (volver a antes de un mensaje del usuario: `Confirm::Rewind` en `menus.rs`, el
    diálogo en `components/menus.rs`). `changes.rs` recuerda qué sesión y turno propuso cada ruta
    (`origins`) y, al llegar bien una escritura, la apunta en esa sesión (`keep_checkpoint`, hasta
    `MAX_CHECKPOINT_BYTES`); el rewind rechaza lo que esos turnos propusieron sin aceptar y pide al
    host devolver los archivos (`Toolbox::restore`, que no toca un archivo cambiado desde entonces).
    Los comandos no se registran: lo que cambien no se deshace.
  - `search.rs` (buscar sesiones por título y mensajes desde la sidebar; las coincidencias se
    calculan al escribir y en momentos concretos, `sessions_changed`, nunca por frame; `groups`
    las filtra), `rename.rs` (renombrar una sesión en su fila, `SidebarRow::renaming`: Enter
    guarda, Escape cancela, y uno a medias se guarda al elegir otra sesión), `palette.rs` (la
    paleta de comandos, `Ctrl+K`: comandos y sesiones filtrados por palabras al escribir, como
    mucho `MAX_SHOWN` filas; su diálogo es `views/palette.rs`), `menus.rs` (menús contextuales y
    borrar sesiones).
  - `servers.rs` (los servidores MCP en Ajustes, desde `ToolServers`: la lista se copia en `pump`
    sólo cuando cambia su versión; un servidor que falla lo avisa en el composer; el formulario
    para añadir y la confirmación de quitar; la vista en `views/mcp.rs`).
  - `yolo.rs` (el modo yolo de una sesión: contesta solo los permisos; el interruptor y el aviso
    están en `components/composer.rs`; ver «Permisos, en detalle»).
  - `costs.rs` (coste por sesión: tokens por modelo, `Session::by_model`, por los precios por
    millón que pone el usuario; sólo proveedores `Cloud`, sin precios de serie).
  - `attention.rs` (con la ventana detrás, una ejecución que acaba o un permiso pedido hace
    parpadear el botón de la barra de tareas y lo cuenta, por `WindowAttention`; `main.rs` le da
    la ventana real con `App::on_window`; el foco llega por `on_window_focus` en la raíz de
    `rune_kit::root`, no en un hijo).
  - `components/tool_card.rs` dibuja una llamada a `update_plan` como su lista de pasos (hecho,
    en curso, pendiente), siempre visible; `scenes.rs` guarda las escenas nuevas (`summary`,
    `plan`) para que `app.rs` no pase de ~800 líneas.
  - `components/inline_diff.rs` (el cambio de un `edit_file` dentro de su tarjeta: diff compacto
    de sólo lectura; `ChangeProposed` lleva el `call` y `pump` guarda su `FileDiff` en
    `card_diffs`), `components/code_copy.rs` (el botón Copy de cada bloque de código, por
    `Markdown::code_frame`; un bloque que aún llega lleva la franja sin botón, para que nada salte).
  - `theme.rs` (único sitio de colores), `keys.rs` (claves estables), `sample.rs` (ejemplos sólo
    para capturas y tests: `with_sample_sessions`; la app real arranca con una sesión vacía y el
    historial).
- `crates/krampsta-app` — binario `krampsta`; construye el catálogo, abre el proyecto inicial
  (`krampsta <carpeta>`, el último abierto o la carpeta actual; en headless, el proyecto y las
  sesiones de ejemplo), conecta los servidores MCP y arranca.

## Historial

Se lee una vez, en `setup` (`load_history`), y llega por `poll` (`take_history`). La UI guarda con
`SessionStore` (sólo encola), en momentos discretos (enviar, fin de herramienta, fin de ejecución,
renombrar, borrar), nunca por delta ni por frame. Con almacén, los ids de sesión salen de
`IdGen::from_clock` para no repetir uno guardado. Las ejecuciones headless (`rune::is_headless()`)
no abren almacén. Un cambio en la forma en disco sube `format::VERSION`; los tests usan
`MemoryStore` o `tempfile`, nunca la carpeta real.

## Compactación

Antes de cada ejecución, `AgentHost::start` llama a `compact::before_run`: si lo que mandarían los
motores (`replay::estimated_bytes`, desde el último resumen) pasa de `COMPACT_AT` (¾ de
`HISTORY_BUDGET`), todo lo anterior a los últimos `KEEP_TURNS` turnos va como transcript
(`TRANSCRIPT_MAX`, lo más nuevo si no cabe) a una petición oculta al mismo motor, sin
herramientas ni audiencia: su texto no llega a la UI, sus tokens sí cuentan. La respuesta llega como
`AgentEvent::Compacted`, que `Session::apply` inserta como mensaje `MessageRole::Summary` antes
del primer mensaje que se queda entero; los motores mandan el resumen como instrucciones
(`SUMMARY_HEADER`) y omiten lo que cubre (`replay::start`). El usuario sigue viendo todo; el chat
marca dónde empieza el resumen (`components/summary.rs`, escena `summary`). Si el resumen falla,
se avisa por stderr y la ejecución sigue sin él (los motores recortan); sólo la cancelación la
para. Un test que quiera ver el recorte de un motor con un historial largo llama al motor
directamente (`RunContext::detached`), no al host.

## Streaming

El texto en streaming no se pinta según llega: `pace.rs` (`Pacer`) estima a qué ritmo llega y lo
suelta a ese ritmo desde `poll`, con una reserva de ~0,25 s que cubre los huecos entre ráfagas
(`tx.wake_after(Duration::ZERO)` mientras queda texto), por palabras enteras, con
`Markdown::reveal` para el fundido y el chat como growth stack con `edge::WORD_FADE` (los bordes
`FEATHER` recortan la fila a su altura animada y el texto salía línea a línea). Una escena que
prepara una respuesta a medias llama a `catch_up` para mostrarla. Para ver cómo sale, una tira de
fotogramas: `krampsta --scene streaming --frames 16 --start 0.05 --out DIR --strip tira.png --crop
280,140,470,200 --every 1 --cols 2`; una captura suelta no enseña animaciones.
`KRAMPSTA_TRACE_PACE=1` registra métricas de ritmo por fotograma en stderr, sin contenido.

## Permisos, en detalle

Antes de ejecutar, `Toolbox::check` dice `Allow`, `Ask` o `Deny`. Con `Ask`, el contexto emite
`ApprovalRequested` y espera (cancelable) a que la UI llame a `AgentHost::answer`; «siempre»
guarda la regla con `Toolbox::remember`. Las reglas viven en `permissions.json` (carpeta de
configuración del usuario):

- Comandos: `deny` gana y mira cada parte de un comando encadenado; una regla `*` de `allow` sólo
  cubre comandos simples. Cada comando deja una línea en `commands.log` (qué, dónde, quién lo
  permitió, cómo acabó). Los comandos no heredan las claves de los proveedores de IA
  (`command::MODEL_KEYS`); una clave nueva que lea Krampsta del entorno se añade ahí.
- Herramientas que no son comandos ni del `Workspace` (las de un servidor MCP): siguen `tools`
  (`allow`/`deny` por nombre o prefijo `*`); las aplica el `Workspace` o, sin carpeta,
  `Permissions` (un `Toolbox` sin herramientas). Para un nombre `mcp__…`, la `ToolSet` toma un
  `NotNeeded` como `Ask`: un toolbox que no conoce la herramienta nunca la deja pasar sin preguntar.
- Páginas web: `fetch_url` pregunta por sitio; «siempre» guarda `fetch_url:<host>` (sólo ese
  sitio) y `deny` gana. Una redirección a otro sitio se para y el modelo tiene que pedir esa URL,
  que pregunta por su cuenta.
- Secretos: `read_file` de un archivo de secretos (`secrets.rs`, por su nombre) pregunta;
  «siempre» guarda la regla `read_file:<ruta>`, sólo para ese archivo; `search` se los salta.
  `.gitignore` oculta un archivo en listados, no de una lectura.
- Si `permissions.json` no se puede leer, todo pregunta y nada se guarda encima.
- Modo yolo (`Session::yolo`, guardado con la sesión; `yolo.rs` en la UI): la UI contesta sola,
  con `Decision::Yolo` (→ `Approval::Yolo`, «yolo mode» en `commands.log`), cada
  `ApprovalRequested` de una sesión en yolo, y al activarlo contesta lo que su ejecución ya
  esperaba. El agente no sabe nada del modo: un `Deny` nunca pregunta, así que sigue ganando. Los
  cambios que propone una sesión en yolo se escriben solos (`changes.rs`: `stage_proposal_from`
  marca la ruta, `write_auto` la escribe desde `pump` cuando no hay otra escritura en marcha, por
  el mismo `Toolbox::write` que «Accept»); si la escritura falla, se muestra y no se reintenta.

## Motores y proveedores, en detalle

- `run_command` corre como árbol de procesos que se mata entero (Job Object / grupo de procesos,
  con `process-wrap`) al vencer el tiempo, al cancelar o al salir su shell. La salida va a la UI
  por lotes (`Call::output`, cada 50 ms o 16 KB) y al modelo como cabeza + cola acotadas.
- Instrucciones: todo motor las forma con `steps::instructions(&ctx)` (las de Krampsta, la pista
  de herramientas y, con herramientas, `ctx.guidance()`: lo que dice el proyecto abierto, no las
  reglas de Krampsta). Es sólo texto: no concede permisos. Un `Toolbox` que envuelve a otro (p. ej.
  para sumar herramientas MCP) reenvía su `guidance`.
- El usuario elige proveedor y modelo en la app; cambiar de motor afecta a las ejecuciones nuevas,
  no a la que está en marcha. Las listas de modelos se piden con `AgentHost::fetch_models` y
  llegan por `poll`, nunca desde `view`.
- Cuentas (ChatGPT, Copilot): lo que dependa de APIs no públicas va en su propio módulo
  (`chatgpt.rs`, `copilot.rs`). Un refresh token rotado se guarda en el llavero al momento. En el
  llavero sólo secretos pequeños y duraderos (Windows limita cada entrada a 2560 bytes); los
  tokens de corta duración se renuevan en memoria. Las llamadas al llavero desde async van por
  `spawn_blocking`. Los tests usan `MemoryStore` y un servidor falso, nunca el llavero real.
- Claves de API guardadas desde la app: `EngineCatalog::set_api_key` (llavero), y el proveedor lo
  anuncia con `Provider::api_key` (`KeySource`). La guardada manda sobre la variable de entorno.
  Los proveedores con clave están en `KEYED` (`catalog.rs`: entrada del llavero, variable y aviso),
  hoy OpenAI y Anthropic; en Ajustes cada uno tiene su propio campo (`KeyField::draft`).
- Para ver qué manda de verdad un backend, `KRAMPSTA_TRACE_SSE=1` escribe en stderr el tipo de
  cada evento del motor Responses (y los nombres de campo de los que ignora), nunca su contenido.
  Pide al usuario esa traza antes de cambiar la petición a ciegas.
- Para grabar fixtures, `KRAMPSTA_RECORD_SSE=<carpeta>` guarda cada stream tal cual (un `.sse` por
  petición, todos los motores). Lleva la conversación entera: el usuario lo activa, y la grabación
  se revisa (y se recorta lo personal o cifrado que no haga falta) antes de versionarla en
  `crates/krampsta-providers/tests/fixtures/`.
- La petición a ChatGPT imita a Codex CLI (`github.com/openai/codex`, `core/src/client.rs`): la
  forma de cada modelo sale del catálogo `codex/models` (`use_responses_lite`,
  `default_reasoning_level`, `supports_reasoning_summary_parameter`). Un modelo Lite exige su
  cabecera y `parallel_tool_calls: false` (si no, HTTP 400). Comprobado con trazas: `gpt-6.2-sol`
  es Lite, con esfuerzo `low` por defecto, y no emite elementos de razonamiento con ningún
  esfuerzo (probados `low` a `max`; `ultra` es un modo de Codex que la API rechaza): el backend no
  lo expone para ese modelo. No lo persigas cambiando la petición.

## Motores: el bucle de herramientas

- Un motor manda `ctx.tool_specs()` al proveedor y ejecuta cada llamada con `ctx.run_tool(..)`
  (hilo bloqueante, cancelable, emite la tarjeta); las de un paso van a la vez con
  `steps::run_calls`, y sus tarjetas terminan en el orden en que acaben (los resultados vuelven
  al modelo en orden). El bucle (pedir → herramientas → resultados → pedir) vive en cada motor,
  porque el formato es de cada API. Los turnos anteriores llevan sus llamadas respondidas (de
  `Message::tool_calls`/`tool_results`), cada una seguida de su resultado y después el texto;
  `krampsta_agent::replay` las recorta por antigüedad: los últimos `RECENT_TURNS` turnos, hasta
  `RECENT_OUTPUT_MAX` por resultado y `RECENT_TURN_MAX` por turno; los anteriores, una línea y
  sin argumentos largos; pasados `FORGET_AFTER`, sólo el texto. Una llamada sin resultado (se
  paró la ejecución) no se manda. `trim_history` sigue quitando turnos enteros por presupuesto.
- La última petición de una ejecución (`steps::is_last`, la número `MAX_STEPS`) va con
  `tool_choice: "none"` y la nota `steps::WRAP_UP` al final del último resultado: el modelo
  responde con lo que tiene en vez de acabar en error. Sólo si aun así pide herramientas falla
  con `too_many_steps`.
- Espera con `ctx.sleep(..)` o futuros cancelables; `usage` se emite también si la petición falla
  tras contar los tokens. Devuelve `Ok(())`, `Err(AgentError::..)` o `Err(AgentError::Cancelled)`.
- `edit_file` devuelve un `FileChange` (antes/después) y guarda la propuesta como pendiente: las
  lecturas y ediciones siguientes del agente la ven. Aceptar o rechazar un bloque es
  `splice_lines` (lineal) sobre los dos textos; `Toolbox::write` escribe de forma atómica.
- El runtime del `AgentHost` usa `enable_all()`: HTTP necesita el driver de I/O, no sólo el de tiempo.

## Rust idiomático, en detalle

- `Arc`/`Rc` sólo cuando el modelo de propiedad lo exige; `Copy` pequeños, por valor. Si un dato
  se comparte mucho, `Arc<str>` / `Arc<[T]>`.
- Evita asignar de más: `_or_else` en vez de `_or` con valores costosos, `with_capacity` cuando
  se sabe el tamaño, reutiliza buffers en bucles (p. ej. el parser SSE).
- Retrasa el `Box` hasta una frontera de diseño; `dyn` para motores, catálogo y contratos.
- Nada de `block_on` en tokio. Un `expect` en tests explica el invariante.

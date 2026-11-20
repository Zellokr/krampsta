---
name: verify-ui
description: Comprobar visualmente la interfaz de Krampsta (crates/krampsta-ui) con capturas headless de Rune y revisarlas antes de dar un cambio por terminado. Úsala después de tocar cualquier vista, componente, tema, layout o escena, cuando el usuario pida ver cómo queda la app, o antes de cerrar una tarea que cambie la UI. Compilar y pasar los tests no prueba que la interfaz esté bien.
---

# Verificar la interfaz de Krampsta

Krampsta se dibuja con Rune, que renderiza headless con el mismo renderer que la ventana y un
reloj virtual: las capturas son deterministas. Este ciclo es el que encontró los fallos
visuales reales del proyecto (título bajo la píldora de estado, Markdown crudo en la sidebar,
diff cortado en el panel estrecho, paneles que aplastaban el chat a 800×560). Síguelo entero.

## 1. Validación de código primero

Todo debe pasar antes de mirar capturas (desde la raíz del repo):

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Si `fmt --check` falla, ejecuta `cargo fmt --all` y repite.

## 2. Renderizar todas las escenas

```bash
bash scripts/snapshots.sh
```

Escribe en `assets/screenshots/`: `krampsta` (estado inicial), `empty`, `changes-empty` (panel de cambios vacío), `changes` y `change-failed` (cambios propuestos con Accept/Reject por archivo y por bloque, y una escritura fallida), `command` (comando en marcha con su salida en directo), `approval` (diálogo para permitir un comando), `running`, `tools`
(tarjetas de herramienta: abierta, fallida y en curso), `thinking` y `reasoning` (razonamiento
mientras piensa y abierto al terminar), `completed`,
`failed`, `cancelled`, `settings`, `api-key` y `api-key-saved` (clave de OpenAI escrita y guardada), `signed-in`, `sign-in` (diálogo con el código), `sign-in-failed` (el fallo, con «Try again»), `session-menu`, `message-menu` y `delete-session` (menús contextuales y la confirmación de borrado), `light`,
`provider-picker`, `effort-picker` (esfuerzo de razonamiento, con ChatGPT), `model-picker`, `model-error`
(selectores abiertos y servidor local caído), `projects` (sesiones de tres proyectos, una plegada), `no-project` y `opening-project` (fila del proyecto sin carpeta y abriendo otra), `narrow` (800×560) y `send-message` (script real:
escribe en el composer y pulsa Enter), más `krampsta-tree.json` (árbol de elementos, no versionado).

Para una escena suelta:

```bash
./target/debug/krampsta.exe --snap out/x.png --scene <escena> --size 1200x800 --scale 1
./target/debug/krampsta.exe --list-scenes
./target/debug/krampsta.exe --tree out/tree.json --scene <escena>     # rects, textos, claves
```

Detalles que ya costaron una iteración:

- **El binario usa el catálogo del entorno**, no `KrampstaApp::with_mock`. `snapshots.sh` fija
  `KRAMPSTA_PROVIDER=mock`; si lanzas `--snap` a mano, exporta lo mismo o la captura dependerá
  de las claves de tu máquina.
- **Krampsta abierto bloquea `target/debug/krampsta.exe`** en Windows («Acceso denegado» al compilar).
  No lo cierres sin preguntar: `CARGO_TARGET_DIR=target/snapshots bash scripts/snapshots.sh`
  compila y captura en otra carpeta.
- **Desplegables abiertos**: revisa que el listbox no corte etiquetas (mide 160 pt como
  mínimo) y que el trigger no se salga del panel (en `narrow` sobre todo).

- **Tamaños estrechos llevan `--at 1.0`.** Los paneles se pliegan en `on_layout`, después del
  primer frame; sin `--at` la captura muestra el layout aún sin plegar.
- **El tema es estado de la app, no del sistema.** `--theme light` no cambia nada (Krampsta es
  oscuro por defecto); usa `--scene light`. Las escenas no se combinan: para ver otra escena
  en claro, añade una escena que haga ambas cosas.
- **Los scripts teclean ~30 caracteres por segundo.** Un `key Enter` debe ir después de que
  termine el `type`, o se envía el texto a medias (ver `scripts/send-message.json`).
- **El MockAgent tarda tiempo real**, no virtual: una captura tras enviar mostrará `Running`.
  Para estados finales usa las escenas preparadas (`completed`, `failed`, `cancelled`).
- Rune escribe sus estadísticas por stderr; no es un error.

## 3. Mirar las capturas

Abre cada PNG con la herramienta de lectura de imágenes y míralo entero. No des por buena una
escena que no has mirado. Repasa en cada una:

1. **Solapes**: título de la barra contra la píldora de estado y el botón de cambios; texto
   contra iconos; paneles contra el contenido.
2. **Texto cortado o mal partido**: palabras sueltas por línea (columna demasiado estrecha),
   títulos sin «…», código que se sale del bloque.
3. **Contenido crudo**: `**`, `` ` ``, `_` visibles fuera del Markdown (previews, títulos).
4. **Fuera de la ventana**: composer, píldora y transcript dentro de los bordes; nada tapado
   por la sidebar o el panel de cambios.
5. **Estados**: cada uno se distingue (spinner en `running`, punto verde en `completed`, tarjeta
   con borde rojo en `failed`, «Run cancelled.» en `cancelled`) y la fila de la sidebar lo refleja.
6. **Tema**: solo tokens del kit OpenCode (`theme.rs`); nada de colores sueltos. Contraste del
   texto secundario legible en `light` y en oscuro.
7. **Coherencia con Rugents**: Krampsta usa el mismo tema; si una pieza equivalente existe en
   Rugents (tarjetas de herramienta, errores), debe parecerse (`../rugents/docs/screenshots/`).

Si dudas de una medida, abre el árbol (`--tree`) y compara los `rect`/`visible` de las claves
`krampsta-*` en vez de fiarte del ojo.

## 4. Comparar antes y después

Las capturas versionadas son la referencia. Tras `scripts/snapshots.sh`:

```bash
git status --short assets/screenshots/
```

Como el render es determinista, solo deben aparecer modificadas las capturas de las pantallas
que querías cambiar. Una captura modificada que no esperabas es una regresión: mírala.

## 5. Cerrar

- Corrige, vuelve al paso 1 y repite hasta que todo esté limpio.
- Si el cambio añade un estado visual nuevo, añade su escena a `SCENES` en
  `crates/krampsta-ui/src/app.rs` y a la lista de `scripts/snapshots.sh`.
- Si añade una interacción, cúbrela en `crates/krampsta-ui/tests/ui.rs` (clic por clave
  `krampsta-*`, `Ime::Commit` para teclear, `pump_until_finished` para el agente).
- Al informar, di qué escenas miraste y qué corregiste; si no pudiste renderizar (sin GPU),
  dilo explícitamente.

## Animaciones (streaming)

Una captura es un instante: no enseña si algo sale fluido. Para el texto en streaming, saca una
tira de fotogramas consecutivos de la escena `streaming` (una respuesta recién llegada que el
pacer va soltando con el reloj virtual) y mírala:

```bash
./target/debug/krampsta.exe --scene streaming --size 1200x800 --scale 1 --frames 16 --fps 60 \
  --start 0.05 --out out/frames --strip out/tira.png --crop 280,140,470,200 --every 1 --cols 2
```

Debe avanzar unas palabras en cada fotograma, sin fotogramas repetidos ni líneas que aparezcan
de golpe. Para la ventana real (no headless), `--native --trace run.jsonl --frames 600 --script
…` guarda la marca de tiempo de cada fotograma: los intervalos deben ser los del monitor.

# ccdesk

Cliente de escritorio ligero (Tauri v2 + Svelte) para trabajar con varias sesiones de
**Claude Code** fuera de la terminal, sin perderlas al cerrar la ventana.

- Las sesiones son **ventanas de una sesión del tmux normal** (por defecto `claude`, se cambia
  en Preferencias). Desde cualquier terminal o por SSH, `tmux a -t claude` da las mismas
  sesiones que el cliente. La app sólo lee y manda teclas: cerrarla no cierra nada.
- Conversación renderizada desde el transcript JSONL (markdown, código resaltado, tablas),
  con botones **Copiar** (HTML + markdown), **Texto** (sin formato) y **MD**.
- Caja de entrada propia: el texto se **teclea** (troceado, Ctrl+J para los saltos de línea),
  no se pega, para que Claude Code no lo pliegue como «[Pasted text]» ni le llegue al modelo
  como texto pegado. Historial con ↑/↓ (el de la TUI). La **terminal real** (xterm.js en una
  sesión tmux agrupada propia) se abre sola cuando Claude pide permiso o pregunta algo.
- **To-do** (panel derecho, por sesión): apunta lo que se te ocurra sin interrumpir y envíalo
  cuando quieras. El botón «To-do» de la caja (Alt+Enter) pasa ahí lo escrito.
- **Subagentes**: tarjeta propia en la conversación (estado, progreso en vivo, encargo,
  resultado, transcript completo en panel lateral) y, mientras trabajan, en árbol bajo su
  sesión en el panel izquierdo.
- Panel lateral: orden por recencia, **arrastrar para reordenar**, **fijar** arriba,
  renombrar, detener/reanudar (`claude --resume`) y **auto-detención** por inactividad.
- Avisos: spinner (trabajando), `!` (requiere atención), punto azul (respuesta sin leer).
- Barra inferior: modelo, effort, contexto, coste y cuotas 5h/7d.
- Recuerda tamaño, posición y maximizado de la ventana (se guarda al cambiar, no sólo al cerrar).
- Sesiones «externas» (claude vivos fuera de ccdesk) en un grupo aparte, sólo lectura.
  **Adoptar** mueve su ventana de tmux a la sesión de ccdesk sin reiniciar el proceso.
  Aviso «⚠» si una sesión está abierta en varios procesos a la vez.

## Cómo funciona

```
tmux (servidor normal) ─ sesión «claude» ─┬─ ventana claude:1  claude --session-id …
                        (tmux a -t claude) ├─ ventana claude:2  …   (@ccdesk_key = cc-xxxxxxxx)
                       sesión «claude-gui» └─ (agrupada, sólo para la terminal de la app)
ccdesk (Rust) ─┬─ ~/.claude/sessions/<pid>.json   estado: idle/busy/waiting/blocked
               ├─ ~/.claude/projects/…/<sid>.jsonl transcript → conversación
               ├─ ~/.cache/ccdesk/status/<sid>.json volcado del statusline → barra inferior
               ├─ ~/.config/ccdesk/state.json      orden, pins, nombres, ajustes
               └─ tmux send-keys (tecleado) / move-window / attach (pty)
```

Una sesión detenida no ocupa memoria: sólo queda su transcript. Ojo con
`cleanupPeriodDays` de Claude Code (30 días por defecto): pasado ese plazo el transcript
se borra y la sesión ya no se puede reanudar.

## Requisitos

- `tmux` ≥ 3.2, `claude` en el `PATH` (o `CCDESK_CLAUDE=/ruta/claude`).
- Compilar: Rust estable, Node 18+, `libwebkit2gtk-4.1-dev`, `librsvg2-dev`.
- **Sensor del statusline** (para la barra inferior), al principio de `~/.claude/statusline.sh`,
  justo después de `input=$(cat)`:

  ```bash
  _ccd_sid=$(printf '%s' "$input" | jq -r '.session_id // empty' 2>/dev/null)
  if [[ "$_ccd_sid" =~ ^[0-9a-f-]{36}$ ]]; then
    _ccd_dir="${HOME}/.cache/ccdesk/status"; mkdir -p "$_ccd_dir" 2>/dev/null
    printf '%s' "$input" > "$_ccd_dir/.$_ccd_sid.tmp" 2>/dev/null \
      && mv -f "$_ccd_dir/.$_ccd_sid.tmp" "$_ccd_dir/$_ccd_sid.json" 2>/dev/null
  fi
  ```

## Compilar y ejecutar

```bash
npm install
npm run tauri dev        # desarrollo (recarga en caliente)
npm run tauri build      # release: src-tauri/target/release/ccdesk + .deb en bundle/deb/
```

## Lanzar sesiones desde fuera (cron, terminal)

```bash
bin/cc-launch -C ~/ia -n todo -- --model sonnet "/todo-list"
```

Crea una ventana en la sesión tmux de ccdesk (en el servidor tmux normal; el nombre se cambia en
Preferencias y es «claude» si no se ha tocado) y aparece sola en la app. Imprime
`<clave> <session-id> <ventana>`. Para entrar desde una terminal: `tmux a -t claude`.

## Atajos

| Atajo | Acción |
|---|---|
| Ctrl+T | Nueva sesión |
| Ctrl+Av Pág / Ctrl+Re Pág | Sesión siguiente / anterior |
| Ctrl+º | Mostrar u ocultar la terminal |
| Enter / Mayús+Enter | Enviar / nueva línea |
| Esc (con Claude trabajando) | Interrumpir |
| Ctrl+Mayús+C | Copiar la selección sin formato (también en la terminal) |
| ↑ / ↓ | Historial de mensajes (el de la TUI, por proyecto) |
| Alt+Enter | Guardar lo escrito como to-do |
| Ctrl+Enter (en un to-do) | Enviar el to-do a la sesión |
| Ctrl+Mayús+O | Mostrar u ocultar el panel To-do |
| Ctrl+, | Preferencias |

## Limitaciones conocidas

- `~/.claude/sessions/*.json` y el formato JSONL no son API pública: una versión nueva de
  Claude Code puede cambiarlos (el código que los lee está aislado en `registry.rs` y
  `transcript.rs`).
- Al reanudar se recuperan modelo y effort del último volcado del statusline; otros flags
  de arranque no.
- `tmux a` a secas entra en la sesión usada más recientemente: si tienes otras sesiones en el
  servidor (o la terminal de la app abierta, `claude-gui`), usa `tmux a -t claude`.
- Las sesiones creadas con versiones anteriores siguen en el servidor antiguo `tmux -L cc`
  hasta que se detienen y se reanudan.
- Las sesiones externas lanzadas con `claude --bg` no se pueden adoptar automáticamente.

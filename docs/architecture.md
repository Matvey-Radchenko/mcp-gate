# Architecture decisions

The first five numbered decisions describe the initial Chrome release. The current
generic ownership/protocol contract is in [configurable-runtime.md](configurable-runtime.md),
including shared backends and the later resource/prompt catalog support.
The opt-in shared backend recovery contract is documented in
[backend-recovery.md](backend-recovery.md). Existing configurations retain manual
recovery; approved local API backends can discard a failed process generation
without replaying the interrupted action.

## 001 — Один Rust gateway, ленивый Node на активную MCP-сессию

Проблема: stdio-сервер загружается на каждую задачу, даже если браузер не нужен.
Переключение всех клиентов на один Chrome DevTools context смешало бы состояние
вкладок и performance trace. В upstream есть общий module-level browser.

Решение: singleton HTTP frontend на Rust, отдельный процесс неизменённого upstream
только после tools/call. Процессная изоляция разделяет глобальные переменные,
выбранные страницы, browser profiles и trace state. Это уменьшает baseline memory,
но не устраняет необходимую память реально работающих браузеров.

## 002 — Каталог инструментов как проверяемый артефакт

При установке однократно запускается backend, выполняются initialize/tools/list,
затем он закрывается. Браузер не запускается. Снимок содержит полный типизированный
список tool schemas и upstream server info, версию, args и hash entrypoint.
Во время работы discovery обслуживается из снимка. Первый worker сверяет реальный
каталог с ним. Это не криптографическая проверка всего npm-пакета: supply-chain
закрепляется package-lock и integrity npm, а snapshot ловит конфигурационный drift.

## 003 — Владение ресурсами и ошибки

Handler -> Session -> Worker -> RunningService + Child + capacity permit.
Сессии хранятся по SDK-generated session ID, не по client name. Worker создаётся под
per-session mutex. Общий semaphore ограничивает число workers; на перегрузке fail-fast.
Worker Drop запускает cleanup; DELETE завершает серверную сессию SDK и освобождает
handler. Graceful shutdown закрывает stdin и ждёт child; после bounded timeout
посылает TERM/KILL только собственной process group и reap-ит процесс.

После backend crash сессия становится failed. Браузерные действия не переигрываются,
состояние автоматически не «восстанавливается». Отмена передаётся с правильным
upstream request ID. Progress возвращается из originating request scope с исходным
client progress token — токены backend и frontend нельзя считать одинаковыми.

Roots запрашиваются в originating call scope, хранятся отдельно для каждого worker
и возвращаются backend без зависимого запроса в неопределённый SSE-канал.
Изменения roots передаются через notification. Roots — upstream filesystem policy,
не отдельный sandbox операционной системы.

## 004 — Потеря соединения не равна бездействию

SDK generic keep_alive=5 minutes отключён. Middleware считает GET streams, HTTP
responses и собственно MCP tool handlers (последние могут пережить HTTP response).
После потери всех GET соединений начинается grace. Возврат GET отменяет отсчёт;
новые POST/завершения ответов обновляют его. Cleanup возможен только без активных вызовов.
Никогда не открывавшим GET клиентам требуется DELETE или явное административное закрытие.

## 005 — Узкая первая версия

Не строим общий plugin runtime, UI, registry или распределённый session store.
Не изменяем Atlassian, Bitbucket, Playwright. Поддерживаем именно инструменты
закреплённого Chrome DevTools и нужные ему legacy roots/progress. Версию протокола
с бессессионным lifecycle не объявляем до отдельного проектирования изоляции.

## 006 — Рекомендация по совместному использованию backend

Проблема: один stdio-процесс экономит ресурсы, но может разделять между клиентами
выбранную вкладку, cookies, рабочий контекст, подписки или ограничения доступа к файлам.

Решение: команды `share` и `isolate` показывают рекомендацию по сохранённому
каталогу. Правила в `recipes/shareability.json` сопоставляют имя сервера или
пакета с известными рисками, ищут признаки выбора или смены контекста в названиях
инструментов, а также использование клиентских roots. `resources.subscribe = true`
указывает на состояние подписок отдельных клиентов. `tools.listChanged` и
logging часто включены SDK и не считаются сигналами. `readOnlyHint` и другие
аннотации не подтверждают отсутствие состояния. Например, `select_page` в
Chrome DevTools меняет общую выбранную страницу даже с `readOnlyHint`.

Без читаемого каталога результат неизвестен и рекомендуется `session`.
При отсутствии сигналов результат `shared_candidate` требует проверки
оператором. Например, сервер memory может быть кандидатом, если все клиенты
должны работать с общим файлом графа знаний.

Рекомендация сама не меняет конфигурацию. Оператор явно сохраняет режим через
`share` или `isolate` и отдельно перезапускает gateway после завершения текущих
действий. `ownership_source = "operator"` сохраняет этот выбор при обновлении
каталога, которое по-прежнему проходит проверку. Конфигурация gateway не
принимает неизвестные поля, поэтому запись разрешена только когда служба gateway
запущена тем же бинарником mcp-gate, что и команда. При подключении нового клиента
setup подбирает gateway с подходящим режимом. Для клиента без рецепта он может
создать отдельный gateway с режимом `session`.

## Client descriptors

[`src/clients/descriptors.toml`](../src/clients/descriptors.toml) is embedded in the
binary and parsed once. Each entry defines a kebab-case ID, display name, document
format, server section and optional nested server section, expansion syntax, and
connection layout. `Client` is a validated ID with catalog-backed Clap values.
The catalog order keeps CLI choices, discovery and sorted output stable, and
IDs are written directly to the registry. The registry still stores the original
document and binding paths, including the `toml` flag, for restoration and drift
checks. JSON and JSONC use the same tolerant parser and byte-preserving editor.

To add a client that uses an existing layout and expansion family, append a
`[[client]]` entry. List personal locations in discovery order with a home-relative
`path` and optional `os` (such as `macos`, `windows` or `linux`). A location with
`env` resolves `path` relative to that variable, or uses its value as the complete
file path when `path` is omitted. An array of path components uses native separators
between components. Available environment locations for the current OS replace
the home defaults in declaration order. List any project-relative files in `project`.
Discovery visits those files only with an explicit project and gives the personal
scope priority when the paths resolve to the same file.

For `connection.mode = "http"`, the descriptor specifies the URL key, any
static fields such as `type`, and either a headers helper key or a file-backed
Authorization header key. For `connection.mode = "connect"`, setup writes an
absolute `current/mcp-gate` command and a `connect` argument list with the
gateway name and explicit configuration path. The bridge reads the gateway's
private token at launch, so the client entry needs no authorization sidecar.
Claude Desktop uses this mode for its personal `mcpServers` on macOS and Windows
with literal arguments and environment values. [MCP SDK configuration paths](https://py.sdk.modelcontextprotocol.io/get-started/real-host/)

Setup skips entries already launching the bridge. The registry compares saved
managed settings with the live document to detect drift. Executable path
migration covers both headers helpers and versioned bridge commands from the
managed store.

The `claude_local_projects` capability enables Claude Code's personal project entries.
Its physical project keys, local precedence and approval checks are implemented in
[`src/clients/claude.rs`](../src/clients/claude.rs).

## Managed binary store

Setup installs the service executable in `binaries/<version>-<sha256>/`. The
registry and operation journals identify each service by its exact executable
path and SHA-256 digest. Each service stays pinned to this path until setup can
explicitly restart it. That preserves exact rollback and recovery.

Setup stages the executable for client headers helpers and stdio bridges in
`current/`, verifies its digest, and publishes it as `current/mcp-gate`
(`current\mcp-gate.exe` on Windows). Unix uses a hardlink with a copy fallback
and an atomic replacement. Windows stages an independent copy because NTFS
hardlinks share the underlying file stream. [Windows file streams](https://learn.microsoft.com/en-us/windows/win32/fileio/file-streams)

If Windows rejects replacement because the executable is in use, setup renames
it to a unique `mcp-gate.exe.old-*` sibling so the running process can continue.
Setup moves the staged executable into the stable path and attempts to restore
the previous filename if that move fails. After a successful publish, including
one with an unchanged digest, setup makes a best-effort pass over old siblings.
Locked files stay in place until a later publish can remove them.

Setup journals executable path migration in managed and external client entries
together with any saved binding changes. After upgrading, re-run `mcp-gate setup`
once and restart the clients it lists. OpenCode reads its Authorization header
from a file.

Remove retains the binary store and `current/`, including after the last
registered gateway is removed. External client entries can still refer to
the stable executable outside the scanned configuration scope. Any future binary
cleanup must retain all registry references and all versions matching the current
executable digest.

## Stdio bridge

`mcp-gate connect <name>` resolves a local gateway from the managed registry.
`--config` uses the same gateway TOML and private token lookup as `headers`.
The bridge owns one Streamable HTTP session per stdio process. It acquires the
session ID and protocol version during initialization, relays subsequent POSTs
concurrently, and forwards POST and optional GET responses through one stdout
writer. On stdin EOF or a shutdown signal, it cancels its streams and sends a
bounded session DELETE.

## Дальнейшие задачи (не обещания текущей версии)

- Регулярный smoke на обновлениях Chrome и при переходе на другой Node.
- Полноценный parent-death supervisor и тест SIGKILL/orphan recovery.
- Тесты sleep/wake, смены сети и долгой эксплуатации в реальном Codex.
- Сохранение runtime release history и автоматизация атомарного upgrade/rollback.
- Отдельное решение для новой бессессионной MCP-версии.

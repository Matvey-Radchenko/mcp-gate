# Architecture decisions

The numbered decisions below describe the initial Chrome release. The current
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

## Managed binary store

Setup installs the service executable in `binaries/<version>-<sha256>/`. The
registry and operation journals identify each service by its exact executable
path and SHA-256 digest. Each service stays pinned to this path until setup can
explicitly restart it. That preserves exact rollback and recovery.

Setup stages the short-lived Codex and Claude Code headers helper in `current/`,
verifies its digest, and atomically replaces `current/mcp-gate`
(`current\mcp-gate.exe` on Windows). Unix uses a hardlink with a copy fallback.
Windows stages an independent copy so a long-running service cannot hold the
helper's file stream open via a hardlink. Replacement briefly retries sharing
errors from a running helper. [Windows file streams](https://learn.microsoft.com/en-us/windows/win32/fileio/file-streams)

Setup journals the executable
path migration in managed and external client entries together with any saved
binding changes. After upgrading, re-run `mcp-gate setup` once and restart the
clients it lists. OpenCode reads its Authorization header from a file.

Remove retains the binary store and `current/`, including after the last
registered gateway is removed. External client entries can still refer to
the helper outside the scanned configuration scope. Any future binary cleanup
must retain all registry references and all versions matching the current
helper digest.

## Дальнейшие задачи (не обещания текущей версии)

- Регулярный smoke на обновлениях Chrome и при переходе на другой Node.
- Полноценный parent-death supervisor и тест SIGKILL/orphan recovery.
- Тесты sleep/wake, смены сети и долгой эксплуатации в реальном Codex.
- Сохранение runtime release history и автоматизация атомарного upgrade/rollback.
- Отдельное решение для новой бессессионной MCP-версии.

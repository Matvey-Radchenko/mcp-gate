# Architecture decisions

The numbered decisions below describe the initial Chrome release. The current
generic ownership/protocol contract is in [configurable-runtime.md](configurable-runtime.md),
including shared backends and the later resource/prompt catalog support.

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

## Дальнейшие задачи (не обещания текущей версии)

- Регулярный smoke на обновлениях Chrome и при переходе на другой Node.
- Полноценный parent-death supervisor и тест SIGKILL/orphan recovery.
- Тесты sleep/wake, смены сети и долгой эксплуатации в реальном Codex.
- Сохранение runtime release history и автоматизация атомарного upgrade/rollback.
- Отдельное решение для новой бессессионной MCP-версии.

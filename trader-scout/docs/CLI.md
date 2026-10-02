# Контракт CLI

Все имена/опции здесь — требования к будущей реализации. Эти команды еще не установлены и не реализованы данным пакетом.

## 1. Общие правила

Три binaries: `buyer-intersect`, `wallet-rank`, `wallet-stats`. Файл задается `--input path`; stdin — `--input -` или отсутствие `--input` при pipe. Если stdin — TTY и вход не указан, показать понятное usage/error вместо неопределенного ожидания ввода.

Входные форматы: `lines`, `csv`, `jsonl`; `--input-format auto` определяет документированный формат, не делает неявное symbol resolution. Token input — contract/mint address, не тикер. Wallet input — public address. Комментарии/пустые строки разрешены в lines; structured formats парсятся строго. Повторные canonical identities дедуплицируются, count duplicates выводится в stderr/manifest. Неверная строка по умолчанию прекращает запуск до платного сканирования, с номером строки и причиной.

Иллюстрация lines, placeholders надо заменить настоящими адресами:

```text
solana:<MINT_OR_WALLET>
bsc:<0x_ADDRESS>
base:<0x_ADDRESS>
robinhood:<0x_ADDRESS>
```

Если все записи одной сети, разрешены bare addresses и `--chain base`. Конфликт per-line chain и `--chain` — ошибка, а не silent override. `--chain auto` определяет только допустимые однозначные случаи по ARCHITECTURE. `--evm-scope all` означает fan-out в явно включенные mainnet EVM profiles и отдельные chain-address результаты. Testnet требуется выбрать явно.

CSV: заголовки `chain,address`; остальные columns только согласно versioned schema. Для token/wallet batch не смешивать разные типы identities без явного record kind.

## 2. Общие опции

| Опция | Поведение |
|---|---|
| `--input`, `--input-format` | Источник и формат identities |
| `--chain auto\|solana\|bsc\|base\|robinhood` | Сетевой профиль; по умолчанию auto с отказом при неоднозначности |
| `--evm-scope all` | Явный fan-out bare EVM wallet addresses по включенным сетям |
| `--period 30d` | Отчетное окно, по умолчанию 30 дней; несовместимо с явным `--since` |
| `--since TIMESTAMP --until TIMESTAMP` | Окно `[since, until)` в UTC; until по умолчанию captured run time |
| `--finality finalized\|confirmed` | Finalized по умолчанию; confirmed маркируется provisional |
| `--config path` | TOML, env содержит секреты |
| `--store path` | Общий embedded store/cache |
| `--backend hybrid\|indexed\|rpc` | По умолчанию hybrid; provider capability requirements не ослабляются |
| `--format table\|jsonl\|csv\|wallets` | Table по умолчанию; wallets только там, где есть wallet records |
| `--manifest path` | Дополнительный machine-readable run manifest |
| `--offline` | Ноль сетевых обращений; использовать только подходящие сохраненные данные |
| `--resume RUN_ID` | Продолжить совместимый persisted run; проверить fingerprint |
| `--dry-run` | План и оценка объема с uncertainties, без основного сканирования |
| `--max-inflight N` | Global admission cap; не переопределяет более строгие provider limits |
| `--memory-budget-mib N` | Общая целевая граница owned working buffers/caches, с disk spill |
| `--max-requests N` | Budget logical requests, retries включаются |
| `--max-provider-units N` | Budget только для известной billing-unit модели; неизвестная модель не выдается за известную |
| `--max-backfill-days N` | Предохранитель восстановления basis; limit reached не означает полную историю |
| `--allow-partial` | Разрешить явно маркированный неполный shortlist; exit code остается 3 |
| `--no-color`, `--quiet` | Управление presentation/progress, не скрывает ошибки качества |

Finality для разных сетей реализуется profile-specific adapter. Нельзя одинаковым числом confirmations объявить одинаковые гарантии. Requested time и effective canonical head могут отличаться; both сохраняются в manifest.

Прогресс, предупреждения и логи — только stderr. stdout — один выбранный формат. JSONL содержит только валидные JSON-строки без ANSI. Table может сокращать адрес визуально, но `wallets`, CSV и JSONL всегда содержат полный адрес. Текст metadata очищается от terminal escape sequences.

## 3. buyer-intersect

```bash
buyer-intersect \
  --input tokens.txt \
  --since 2026-08-01T00:00:00Z \
  --until 2026-09-01T00:00:00Z \
  --min-token-hits 3 \
  --min-buy-usd 25 \
  --format table
```

Специальные опции:

| Опция | Контракт |
|---|---|
| `--min-token-hits K` | По умолчанию 2; число различных входных AssetKeys, а не buys |
| `--match any\|all` | any — порог K, all — все N distinct input tokens; all конфликтует с явным K |
| `--min-buy-usd AMOUNT` | По умолчанию 0, USD-фильтр отключен; >0 требует execution valuation |
| `--identity chain-address\|evm-address` | По умолчанию chain-address; grouping не подтверждает общего владельца |
| `--entity-map path` | Explicit mapping address→entity; отдельные provenance и grouping label |
| `--include-holdings` | Дополнительный snapshot остатка; не меняет определение исторической покупки |

Меньше двух distinct input tokens — usage error для задачи пересечений. K>N — usage error. Default `any` значит «купил хотя бы K из списка», не «любое число >0».

Результат: wallet/group identity, network(s), hit_count, N, matched AssetKeys, first qualifying buys с tx evidence, buy counts, spend при наличии цены, holdings по запросу, quality. Сортировка: hit_count desc, canonical wallet/group key asc. Дополнительные sort options допустимы только с документированной missing-values policy.

Покупатель, который позже все продал, остается историческим покупателем. Чистый получатель airdrop не становится покупателем. При неполном сканировании токена N не уменьшается. Строгий финальный shortlist не объявляется полным; JSONL содержит status=partial, а plain wallets требует явного `--allow-partial` плюс `--manifest`, чтобы потеря completeness metadata была осознанной.

### Текущее состояние реализации (2026-10-02)

Реализован только срез Solana pump.fun bonding-curve (все входные токены — `solana:<mint>`, нужен `SCOUT_HELIUS_API_KEY`). EVM-вход и вход без ключа завершаются кодом 4. Охват и ограничения печатаются в stderr; stdout содержит только выбранный формат.

Поддерживаемые флаги: `--input`, `--min-token-hits K` (по умолчанию 2), `--format table|jsonl` (по умолчанию `table`), `--max-pages-per-token N`.

`--max-pages-per-token N` (1..=200, по умолчанию 10; ошибка диапазона — exit 2) — бюджет страниц провайдера НА КАЖДЫЙ входной токен (Helius full mode: 100 транзакций на страницу). Ретраи в этот бюджет не входят; их учитывает `--max-requests`. Исчерпание бюджета при непрочитанном курсоре помечает токен `truncated`, запуск `partial`, exit 3. Эффективное значение печатается в блоке scope на stderr и в `run_meta.budget`.

`--max-requests N` (N ≥ 1; без флага — без лимита, но счетчик ведется) ограничивает ВСЕ HTTP-попытки запуска, включая ретраи; `requests_made` печатается в stderr и в `run_meta`. При исчерпании бюджета или при 429 с `Retry-After` больше лимита ожидания (60 с) скан останавливается: прерванный токен — `failed` с `error_kind`, остальные — `not_scanned` с `stop_reason` (`{kind: budget_exhausted|rate_limited, limit?, retry_after_secs?}`), запросы по ним не делаются. Exit: бюджет — 3; rate limit — 3, если данные уже получены, и 4, если не получено ни одной транзакции.

Общие опции §2 (`--since`, `--until`, `--allow-partial`, `--manifest`, `--chain`, `--input-format` и др.) и опции §3 кроме `--min-token-hits` пока не реализованы.

`table`: одна строка на кошелек `<полный адрес> hit_count=N`. `jsonl`: `run_meta`, затем `buyer_match` (по одному на кошелек), затем терминальный `run_summary`. Адреса полные (base58 для Solana), имена сетей как во входном синтаксисе. Неизвестное не равно нулю: у токена со статусом `failed` счетчики `null`.

```json
{"schema_version":1,"kind":"run_meta","run_id":"buyer-intersect-20261002T123456Z","captured_at":"2026-10-02T12:34:56Z","scope":{"chain":"solana","program_id":"6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P","idl_commit":"<git sha>","idl_sha256":"<sha256>","qualification_version":"pump-bonding-curve-buy/idl-e0687ae/v4","recognized":"...","not_decoded":"...","variants":[{"name":"buy","side":"buy","verification":"FixtureVerified"}]},"budget":{"max_pages_per_token":10},"input_tokens":[{"chain":"solana","token":"<MINT_A_BASE58>"},{"chain":"solana","token":"<MINT_B_BASE58>"}],"input_token_count":2,"min_token_hits":2}
{"schema_version":1,"kind":"buyer_match","wallet":{"chain":"solana","address":"<WALLET_BASE58>"},"hit_count":2,"matched_assets":[{"chain":"solana","token":"<MINT_A_BASE58>"},{"chain":"solana","token":"<MINT_B_BASE58>"}]}
{"schema_version":1,"kind":"run_summary","run_id":"buyer-intersect-20261002T123456Z","status":"partial","cancelled":false,"records":1,"incomplete_reasons":["token <MINT_B_BASE58>: scan failed: ..."],"tokens":[{"token":{"chain":"solana","token":"<MINT_A_BASE58>"},"status":"ok","error":null,"transactions_scanned":250,"qualified_buyers":3,"diagnostics":{"decoded_buys":9,"malformed_instructions":0,"unknown_discriminator_instructions":0,"unverified_variant_buys":0,"failed_transactions":1,"positive_delta_without_instruction":0}},{"token":{"chain":"solana","token":"<MINT_B_BASE58>"},"status":"failed","error":"...","transactions_scanned":null,"qualified_buyers":null,"diagnostics":null}]}
```

`run_summary.status` — `complete` или `partial`; `tokens[].status` — `ok`, `truncated` (счетчики — нижняя граница), `failed` (с `error_kind`) или `not_scanned` (с `stop_reason`, счетчики `null`). Строки в примере сокращены/иллюстративны. Записи `wallet_ref`, `wallet_rank`, `wallet_stats`, `wallet_excluded` здесь не выпускаются.

## 4. wallet-rank

```bash
wallet-rank \
  --input wallets.txt \
  --period 30d \
  --top 10 \
  --rank-by realized-net-pnl \
  --profile quality \
  --format table
```

`--top` по умолчанию **20**, N>=1. Если прошли фильтры только 7, вывести 7. Пустой complete результат — нормальный исход.

`--rank-by realized-net-pnl|realized-cost-roi|profit-factor|period-equity-pnl`. По умолчанию realized-net-pnl; definitions и requirements — в ARCHITECTURE. Не сравнивать разные report windows, quote policies или metric definitions в одной колонке. Cross-chain сравнение допускается в USD только при сопоставимом scope/quality; chain сохраняется.

`--profile quality|none`, по умолчанию quality. `--min-closed-episodes 20` и `--min-active-days 7` переопределяют sample gates; можно задать другие quality thresholds через config. `none` не превращает unknown PnL в число. Unknown/invalid metric не участвует в числовой сортировке.

`--exclude-assets tokens.txt` разрешается для отдельной проверки вне discovery token universe. Исключения входят в scope/report hash; это не полный wallet PnL и должно так называться. Нельзя скрыть исключенные убыточные активы и оставить подпись «полная доходность кошелька».

Human output: rank, wallet, chain, realized_net_pnl, realized_cost_roi, closed episodes, win rate, PF, open exposure/valuation status, quality. Exclusion summary включает counts по каждой причине. JSONL содержит итоговые исключения и исходные observations, а не только топ.

### Текущее состояние реализации (2026-10-02)

Срез Solana pump.fun bonding-curve + PumpSwap AMM (программа `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA`, ADR-012; оба IDL закреплены на `e0687ae9`; НЕ декодируются Raydium, Meteora, Orca, Jupiter-маршруты вне этих двух программ) поверх того же анализа, что и `wallet-stats` (ADR-010, PnL в лампортах; нужен `SCOUT_HELIUS_API_KEY`). Флаги: `--input`, `--input-format lines|jsonl` (принимает записи `buyer_match`/`wallet_ref`/`wallet_stats`/`wallet_rank`), `--format table|jsonl`, `--top N`, `--rank-by realized-net-pnl|realized-cost-roi|profit-factor` (`period-equity-pnl` — exit 2 до P5.2), `--profile quality|insider|none`, `--min-closed-episodes`, `--min-active-days`, `--max-trades-per-day`, `--max-mints-per-day`, `--require-no-open`, `--max-pages-per-wallet`, `--max-requests`. `--period <N>d` (1..=365) / `--since` / `--until` — окно анализа ADR-011 (ниже).

Окно (ADR-011): `[since, until)` в unix-секундах UTC. `--since/--until` — строго `YYYY-MM-DDTHH:MM:SSZ` (другие смещения/формы — exit 2); `--period 30d` = `[until − 30·86400, until)`, `until` по умолчанию `as_of` (старт запуска, фиксируется один раз). `--period` несовместим с `--since`; `--until` без `--since/--period`, `until` в будущем и `since >= until` — exit 2. Без флагов окна — прежнее поведение (полная история обязательна). Скан newest-first останавливается после первой страницы, где есть tx с `blockTime < since` (провайдер: `HeliusProvider::with_stop_before_block_time`; страниц за граничной не запрашивается) — это complete для окна; исчерпание бюджета страниц/запросов до границы — `incomplete`, exit 3. Tx вне `[since, until)` отбрасываются до леджера; tx без `blockTime` в просмотренном диапазоне — coverage gap (`incomplete`). `until < as_of` по-прежнему требует прохода от новейшего края до `until` (серверный фильтр времени не предполагается). Инвентарь, предшествующий окну, — `LeftCensored` (эпизоды `left_censored`, не оцениваются и не входят в суммы/ratios); `exclude_unknown_basis` не срабатывает на одну левую цензуру. Для `incomplete` кошельков потолок активности (`insider`) проверяется на полностью наблюдаемых днях (все наблюдаемые дни, кроме самого старого) и добавляется причиной после `incomplete_coverage`; кошелек остается исключенным. `run_meta.window = {since, until, since_unix, until_unix, as_of, as_of_unix, source: period|explicit|none}`; в table первой строкой `# window [...)`. В observed-метриках: `left_censored_episodes`, `left_censored_amount_raw`, `has_left_censored_inventory`. Ledger version `solana-wallet-ledger/3`.

Профили (стартовая исследовательская policy, не статистическая гарантия): `quality` — ≥20 известных закрытых эпизодов, ≥7 активных UTC-дней, без unknown basis; `insider` (P4.3) — ≥5 эпизодов, ≥3 дня, потолок активности ≤30 сделок и ≤10 различных mint на активный день (отсекает HF/sniper-популяцию из P0.7), без unknown basis; `none` — без sample gates, unknown metric не участвует в сортировке, PnL с unknown-эпизодами помечается `known_subset`. Пороги сравниваются точной целочисленной арифметикой.

Открытые позиции с известной себестоимостью не оцениваются (нет источника цен, P5.2): кошелек остается в рейтинге с пометкой `open_exposure: unvalued`; строгий вариант — `--require-no-open`. Это явное отступление от `require_resolved_open_exposure=true` до P5.2.

Каждый входной кошелек попадает либо в `wallet_rank`, либо в `wallet_excluded` со всеми причинами (порядок: provider_error, not_scanned, incomplete_coverage, no_activity, no_pump_activity, unknown_basis, metric_unknown, insufficient_closed_episodes, insufficient_active_days, activity_unknown, activity_ceiling_trades_per_day, activity_ceiling_mints_per_day, open_exposure, below_top_n). Exit: 0 — вся выборка просканирована (пустой рейтинг — норма); 3 — рейтинг по неполной выборке; 4 — нет ключа, EVM-вход или все кошельки с ошибкой; 2 — usage.

## 5. wallet-stats

```bash
cat wallets.txt | wallet-stats --input - --period 30d --format table
```

Никакого неявного top/filtering. Вывести одну карточку/строку на каждый distinct входной WalletKey в порядке первого появления; для `--evm-scope all` — явно раскрытые per-chain строки. Кошелек без активности получает status=no_activity, не исчезает. Кошелек с неполной историей получает карточку с N/A и gaps, не ложные нули.

`--detail summary|full`, по умолчанию summary. Full включает per-token positions, lots/realizations summary, fees, timing, coverage и evidence references. `--sort input|realized-net-pnl` может менять порядок, но не состав списка. Default input.

Пример полей карточки, а не реальные результаты:

```text
Wallet / Chain / Analysis window / As-of
Realized net PnL          observed / known-subset / N/A
Matched-cost ROI         value / N/A + denominator
Closed episodes          valid / censored / open
Win rate / PF            value + sample size + status
Open positions           basis / marked value / price quality
Holding time / activity  medians, distinct assets, active days
Fees                     allocated / overhead / unallocated
Concentration            largest positive-PnL token share
Data quality             scope, gaps, unknown basis, attribution
```

### Текущее состояние реализации (2026-10-02)

Срез Solana pump.fun bonding-curve + PumpSwap AMM, SOL-леджер в лампортах (ADR-010, ADR-012): один FIFO на `(wallet, mint)` по обеим площадкам; сделка PumpSwap атрибутируется кошельку только при сверке его собственных owner-keyed legs (router-forward не атрибутируется; квота, оплаченная другим аккаунтом, — Unknown basis, не 0); в `stats` счетчики сделок по площадкам/вариантам и диагностика `router_forward_trades_not_attributed`, `quote_funded_elsewhere_trades`, `reversed_pool_trades`; `scope` содержит обе программы и оба IDL-пина; НЕ декодируются Raydium, Meteora, Orca, Jupiter-маршруты вне этих двух программ (их движения токенов — разрывы непрерывности, Unknown, не нулевой PnL); нужен `SCOUT_HELIUS_API_KEY`. EVM/смешанный вход и вход без ключа — exit 4 (кошельки не отбрасываются молча). Флаги: `--input`, `--input-format lines|jsonl`, `--format table|jsonl`, `--detail summary|full`, `--sort input|realized-net-pnl`, `--max-pages-per-wallet N` (1..=200, по умолчанию 10), `--max-requests N` (N ≥ 1; без флага — без лимита, но счетчик ведется; все HTTP-попытки запуска, ретраи включены; `requests_made` и `max_requests` печатаются в stderr и в `run_meta`). `--period <N>d` / `--since` / `--until` — окно анализа ADR-011 (семантика как в §4: strict RFC 3339 UTC, `[since, until)`, остановка скана на первой странице старше `since`, бюджет до границы — `incomplete`/exit 3, tx вне окна отбрасываются, tx без `blockTime` — gap, неверное окно — exit 2). Карточка и JSONL показывают `left_censored_episodes` (эпизоды, чей инвентарь предшествует окну: считаются, не оцениваются, не входят в суммы), `left_censored_amount_raw`, `has_left_censored_inventory`, `coverage.transactions_in_window`; таблица — колонку `left_censored` и первую строку `# window [...)`; `run_meta.window` и ledger version `solana-wallet-ledger/3`.

Скан идет newest-first с бюджетом страниц на кошелек: `truncated` = более старая история не просмотрена, начальный инвентарь неизвестен, кошелек получает `status=incomplete` и exit 3. Статусы карточки: `ok`, `no_activity` (0 транзакций), `no_pump_activity`, `incomplete`, `error` (сбой провайдера на этом кошельке; остальные кошельки продолжаются), `not_scanned` (запрос не делался: запуск остановлен раньше). Исчерпание бюджета `--max-requests` или 429 с `Retry-After` больше лимита ожидания (60 с) останавливает скан: прерванный кошелек — `error` с типизированным `error_kind`, остальные — `not_scanned` с `stop_reason` (`{kind: budget_exhausted|rate_limited, limit?, retry_after_secs?}`), `run_summary.stop` повторяет причину; запросы по ним не делаются, кошельки не пропадают. Exit: бюджет — 3; rate limit — 3, если хотя бы один кошелек дал карточку с данными, иначе 4. Все кошельки `error` — exit 4; часть — exit 3. Unknown PnL / unknown-basis — легитимный N/A, не причина для exit 3. Деньги в JSONL — строки (лампорты и SOL с 9 знаками). JSONL: `run_meta`, `wallet_stats` на кошелек, `run_summary`. JSONL-вход принимает `buyer_match`/`wallet_ref`/`wallet_stats`; при наличии `run_meta` без `run_summary status=complete` запуск завершается кодом 3.

## 6. Композиция через stdout/stdin

Безопасный пример с фиксированным периодом и staged files; следующие команды выполняются только при успешном upstream:

```bash
buyer-intersect \
  --input tokens.txt \
  --since 2026-08-01T00:00:00Z --until 2026-09-01T00:00:00Z \
  --min-token-hits 2 --format jsonl > candidates.jsonl &&
wallet-rank \
  --input candidates.jsonl --input-format jsonl \
  --since 2026-08-01T00:00:00Z --until 2026-09-01T00:00:00Z \
  --top 20 --format jsonl > ranked.jsonl &&
wallet-stats \
  --input ranked.jsonl --input-format jsonl \
  --since 2026-08-01T00:00:00Z --until 2026-09-01T00:00:00Z \
  --offline --format table
```

При одном default store и совместимых policies ranking уже должен сохранить требуемые данные для последнего шага. Если часть stats fields запрашивает дополнительные данные, offline возвращает явный cache miss, а не делает сеть.

Direct pipeline также поддерживается:

```bash
set -o pipefail
buyer-intersect --input tokens.txt --format jsonl |
  wallet-rank --input - --input-format jsonl --top 20 --format jsonl |
  wallet-stats --input - --input-format jsonl --format table
```

JSONL input adapter понимает output records предыдущих CLI, берет identities и context, а не пытается парсить таблицу. Для reproducible pipeline upstream run_meta передает effective window/as-of: downstream по умолчанию наследует его при отсутствии собственных временных опций. Явные несовместимые параметры требуют новой оценки с новым manifest, не молчаливого использования старого PnL.

Если есть run_meta, downstream обязан проверить terminal run_summary; отсутствие footer/partial status нельзя считать complete candidate universe. До завершения upstream список можно спулировать на диск; не требуется держать его в RAM. `set -o pipefail` остается обязательным примером shell orchestration.

## 7. JSONL envelope

Фиксировать JSON Schema в P1. Обязательная структура:

```json
{"schema_version":1,"kind":"run_meta","run_id":"example","window":{"since":"2026-08-01T00:00:00Z","until":"2026-09-01T00:00:00Z"},"snapshot_manifest":"example"}
{"schema_version":1,"kind":"wallet_ref","wallet":{"chain":"base","address":"0x1111111111111111111111111111111111111111"}}
{"schema_version":1,"kind":"run_summary","run_id":"example","status":"complete","records":1}
```

Адрес в примере — только синтаксическая иллюстрация. Другие record kinds: `buyer_match`, `wallet_rank`, `wallet_stats`, `wallet_excluded`. Все wallet-bearing results содержат одинаковое поле `wallet`; group records имеют отдельный schema kind, их нельзя автоматически подавать как один адрес. Утилита 2 принимает identities из `buyer_match`/`wallet_ref`/`wallet_stats`; утилита 3 принимает `wallet_rank`/`wallet_ref`/`buyer_match`. Excluded records не становятся ranked identities автоматически.

Большие raw integers/money — strings. Для каждой нетривиальной метрики: value либо null, status, unit, definition/policy id, relevant denominator. No NaN/Infinity. JSONL final summary содержит source completion независимо от metric eligibility. Отсутствие данных по определенной метрике может быть легальным N/A, но operational gap обязательно отражается в run status.

CSV имеет фиксированный versioned column set; quality summary хранится в columns и manifest. `--format wallets` выводит только `chain:full_address` и предназначен для простых pipelines, которым не нужны метрики; metadata loss явно документируется.

## 8. Exit codes

| Code | Значение |
|---|---|
| 0 | Выполнение завершено в declared scope; shortlist может быть пустым/короче N; legitimate N/A не обязательно является operational failure |
| 2 | Ошибка аргументов, формата, неоднозначная сеть, несовместимые конфигурации |
| 3 | Неполное выполнение обязательного scan/coverage contract; данные могут быть частично выведены с маркировкой |
| 4 | Инфраструктура/credentials/storage/capability не позволяют выполнить задачу |
| 130 | Отмена пользователем; durable checkpoint сохранен насколько возможно |
| 141 | Закрыт stdout pipe; без panic/backtrace, с корректной остановкой producer'ов |

Отличать `eligible=false` из-за малой выборки от `not_evaluated` из-за отказа провайдера. Первое может завершаться 0, второе при required input влияет на completion. `--allow-partial` не переписывает exit 3 в success.

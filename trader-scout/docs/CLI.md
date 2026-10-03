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

### Текущее состояние реализации (2026-10-03, ADR-014)

Реализован срез Solana (все входные токены — `solana:<mint>`, нужен `SCOUT_HELIUS_API_KEY`). EVM-вход и вход без ключа завершаются кодом 4. Охват и ограничения печатаются в stderr; stdout содержит только выбранный формат.

**Сторона сделки (ADR-014, решение владельца).** `--side buy|sell|any`, по умолчанию `any`: кошелек попадает на токен T, если у него есть хотя бы одна квалифицирующая операция выбранной стороны по T; `--min-token-hits K` считает различные входные токены (с учетом фильтра стороны). `any` = покупка ИЛИ продажа каждого из K токенов; `--side buy` воспроизводит прежнюю семантику «покупатели» (теперь по всем площадкам). Это изменяет прежнее описание выше («матчились только покупки»): по умолчанию результат — надмножество старого buy-only набора.

**Квалифицирующие операции** (никогда не transfer/airdrop, никогда не router/relayer/fee payer): покупка и продажа на pump.fun bonding curve (FixtureVerified-вариант, decoded `user` = кошелек, owner-keyed net delta токена со знаком стороны); сделка PumpSwap AMM (ADR-012: decoded `user` = кошелек, собственные ноги кошелька сходятся с событиями, сторона в терминах токена, reversed pool инвертируется; router-forward не приписывается); route swap (ADR-013 §2 a–d; доказательство swap-ноги — FixtureVerified-нога pump (curve/PumpSwap) ИЛИ hop события Jupiter v6 `SwapEvent`/`SwapsEvent` с токеном на входе или выходе, ADR-015, только evidence без владения; сторона = знак собственной дельты кошелька по токену). Вариант IdlOnly не квалифицируется: считается, делает покрытие неполным (exit 3). Атрибуция — та же функция, что в ledger (`attribute_transaction_trades`), применяется ко всем кандидатам транзакции (signers ∪ users декодированных ног ∪ владельцы с ненулевой дельтой токена). НЕ декодируются (нижняя граница, названо в `run_meta.scope.not_decoded`): Raydium, Meteora DLMM, Orca Whirlpool, сами площадки-хопы Jupiter и маршруты без ноги pump и без события Jupiter v6 (например, нераспознанные роутеры `DF1ow4ts…`), нетрейдовые инструкции PumpSwap. Диагностика покрытия: у токена `diagnostics.evidence_samples[]` (до 5 образцов в каноническом порядке: `kind` = `malformed_trade_instruction|malformed_event|unknown_discriminator|orphan_event`, `signature`, `slot`, `instruction_index`, `program`, `program_name`, `variant_or_discriminator`, `data_len`, `accounts_len`, `reason`), счетчики `jupiter_malformed_events`/`jupiter_unknown_events`; те же образцы печатаются на stderr (`evidence (up to 5): …`).

**Окно.** `--since/--until/--period` как в ADR-011 (`[since, until)`, UTC RFC 3339 строго `…Z`, `--period Nd` 1..=365; ошибка — exit 2). Без окна скан по токену идет от создания токена (oldest-first), усечение страниц = нет НОВЕЙШЕЙ активности, exit 3 как раньше. С окном — newest-first до границы (`HeliusProvider::with_stop_before_block_time`): токен полон для окна, если граница достигнута или история закончилась; иначе (бюджет страниц) — `partial`/exit 3. Транзакции без `blockTime` до границы — пробел покрытия.

Версия квалификации: `solana-trade-qualification/v7 (curve+pumpswap+route, ADR-014, ADR-009 26-byte track_volume, ADR-015 Jupiter route legs)` (`SOLANA_BUY_QUALIFICATION_VERSION` остается идентификатором правила curve-buy ADR-003 функции `qualify_bonding_curve_buys`).

Поддерживаемые флаги: `--input`, `--min-token-hits K` (по умолчанию 2), `--side buy|sell|any` (по умолчанию `any`), `--since`, `--until`, `--period`, `--format table|jsonl` (по умолчанию `table`), `--max-pages-per-token N`, `--max-requests N`.

`--max-pages-per-token N` (1..=200, по умолчанию 10; ошибка диапазона — exit 2) — бюджет страниц провайдера НА КАЖДЫЙ входной токен (Helius full mode: 100 транзакций на страницу). Ретраи в этот бюджет не входят; их учитывает `--max-requests`. Исчерпание бюджета при непрочитанном курсоре помечает токен `truncated`, запуск `partial`, exit 3. Эффективное значение печатается в блоке scope на stderr и в `run_meta.budget`.

Опции запроса провайдера (проверены live 2026-10-03, см. `docs/p0/measurements/2026-10-03-helius-filters-live.md`; значения по умолчанию — константы `DEFAULT_PAGE_LIMIT_ARG`, `DEFAULT_PROVIDER_STATUS_FILTER`, `DEFAULT_SERVER_WINDOW` в `bins/buyer-intersect/src/main.rs`; `--page-limit 100 --provider-status-filter any --server-window=false` воспроизводят прежний запрос байт-в-байт): `--page-limit N` (1..=1000, по умолчанию 500; `limit` на страницу; бюджет `--max-pages-per-token` остается в СТРАНИЦАХ, транзакционный бюджет токена = `pages × page-limit`; страницы свыше 500 tx поднимают лимит размера ответа ~20 KB/tx, максимум 64 MiB), `--provider-status-filter any|succeeded` (по умолчанию `succeeded`; при `succeeded` неуспешные tx невидимы), `--server-window[=true|false]` (по умолчанию включен; при заданном окне дополнительно шлет `filters.blockTime {gte: since, lt: until}`, newest-first граница остается; без окна не действует). Эффективные значения — в `run_meta.provider_options` и строке `provider options:` блока scope на stderr.

`--max-requests N` (N ≥ 1; без флага — без лимита, но счетчик ведется) ограничивает ВСЕ HTTP-попытки запуска, включая ретраи; `requests_made` печатается в stderr и в `run_meta`. При исчерпании бюджета или при 429 с `Retry-After` больше лимита ожидания (60 с) скан останавливается: прерванный токен — `failed` с `error_kind`, остальные — `not_scanned` с `stop_reason` (`{kind: budget_exhausted|rate_limited, limit?, retry_after_secs?}`), запросы по ним не делаются. Exit: бюджет — 3; rate limit — 3, если данные уже получены, и 4, если не получено ни одной транзакции.

Общие опции §2 (`--allow-partial`, `--manifest`, `--chain`, `--input-format` и др.) и опции §3 кроме `--min-token-hits` и `--side` пока не реализованы.

`table`: одна строка на кошелек `<полный адрес> hit_count=N <mint>=B/S <mint>=S …` (маркеры `B` — наблюдалась покупка, `S` — продажа, `B/S` — обе; только выбранные `--side`). `jsonl`: `run_meta` (добавлены `side`, `window`, `scan_order`, `scope.amm_*`, `scope.qualification_version` v5), затем `buyer_match` (по одному на кошелек; kind и `matched_assets` прежние, добавлен `matched_tokens[]`: `token`, `sides`, `buy_count`/`sell_count`, `first_buy`/`first_sell` = `{signature, slot, venue, variant}` с наименьшим `(slot, tx index)`), затем терминальный `run_summary` (у токена добавлены `qualified_sellers`, `qualified_wallets`, `transactions_in_window`; `diagnostics.qualified_ops` по площадкам `bonding_curve|pump_amm|route` × `buys|sells`, `router_forwards_not_attributed`, `route_rejections`, `idl_only_trades`, `amm_unreconciled`, `trades_without_matching_delta`, `malformed_trades`, `orphan_events`). Адреса полные (base58 для Solana), имена сетей как во входном синтаксисе. Неизвестное не равно нулю: у токена со статусом `failed` счетчики `null`.

```json
{"schema_version":1,"kind":"run_meta","run_id":"buyer-intersect-20261002T123456Z","captured_at":"2026-10-02T12:34:56Z","scope":{"chain":"solana","program_id":"6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P","idl_commit":"<git sha>","idl_sha256":"<sha256>","qualification_version":"solana-trade-qualification/v6 (curve+pumpswap+route, ADR-014, ADR-009 26-byte track_volume)","side":"any","window":{"source":"none"},"recognized":"...","not_decoded":"...","variants":[{"name":"buy","side":"buy","verification":"FixtureVerified"}]},"budget":{"max_pages_per_token":10},"input_tokens":[{"chain":"solana","token":"<MINT_A_BASE58>"},{"chain":"solana","token":"<MINT_B_BASE58>"}],"input_token_count":2,"min_token_hits":2}
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


**ADR-013 (ledger/4, wallet-rank).** `--quote sol|usdc|usdt` (по умолчанию `sol`) выбирает единицу, в которой считаются PnL/ROI/PF-ранжирование и gate `min_closed_episodes` (known closed эпизоды именно этой единицы); единицы не смешиваются и не конвертируются, gate `unknown_basis` остается по кошельку целиком. JSONL: `run_meta.rank_quote_unit`, `run_meta.ledger_scope`, `metrics.quote`, `metrics.closed_known_in_quote`, `metrics.quote_units[]` (по каждой единице: closed_known, wins/losses/breakeven, `realized_trade_pnl`/`consumed_acquisition_basis` как `{raw, decimal}` (SOL 9 dp, USDC/USDT 6 dp, null без known closed эпизода), `realized_cost_roi` как точный рационал, win_rate, profit_factor), `metrics.route` (счетчики route swaps); `metrics.realized_net_pnl` = `{status, unit, raw, decimal}` в единице ранжирования, поля `lamports/sol/sol_exact` заполнены только при `unit=sol`. Прежние SOL-поля (`realized_trade_pnl`, `consumed_acquisition_basis`, `realized_cost_roi`, `profit_factor`, `failed_trade_fees`) сохраняют SOL-смысл; `closed_episodes_known/wins/losses/breakeven/win_rate` — по всем единицам. Таблица: колонка `realized_net_pnl_<unit>` и `# ... quote=<unit>`. Route swap (кошелек подписывает, FixtureVerified pump-нога, ровно один токен и один quote-актив SOL/USDC/USDT с противоположными знаками, остальные `user` ног — не подписанты с нулевым нетто) бронируется по собственным дельтам кошелька; цена события чужой ноги больше не используется как цена кошелька (§1, исправляет баг ledger/3 с SOL-ценой USDC-сделок). Version `solana-wallet-rank/2`.
Срез Solana pump.fun bonding-curve + PumpSwap AMM (программа `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA`, ADR-012; оба IDL закреплены на `e0687ae9`; НЕ декодируются Raydium, Meteora, Orca; hop-события Jupiter v6 (`SwapEvent`/`SwapsEvent`) — только доказательство swap-ноги для route swap, ADR-015) поверх того же анализа, что и `wallet-stats` (ADR-010, PnL в лампортах; нужен `SCOUT_HELIUS_API_KEY`). Флаги: `--input`, `--input-format lines|jsonl` (принимает записи `buyer_match`/`wallet_ref`/`wallet_stats`/`wallet_rank`), `--format table|jsonl`, `--top N`, `--rank-by realized-net-pnl|realized-cost-roi|profit-factor` (`period-equity-pnl` — exit 2 до P5.2), `--profile quality|insider|none`, `--min-closed-episodes`, `--min-active-days`, `--max-trades-per-day`, `--max-mints-per-day`, `--require-no-open`, `--max-pages-per-wallet`, `--max-requests`, `--page-limit`, `--server-window`, `--token-accounts` (опции запроса провайдера см. §5; проверены live 2026-10-03, `docs/p0/measurements/2026-10-03-helius-filters-live.md`). `--period <N>d` (1..=365) / `--since` / `--until` — окно анализа ADR-011 (ниже).

Окно (ADR-011): `[since, until)` в unix-секундах UTC. `--since/--until` — строго `YYYY-MM-DDTHH:MM:SSZ` (другие смещения/формы — exit 2); `--period 30d` = `[until − 30·86400, until)`, `until` по умолчанию `as_of` (старт запуска, фиксируется один раз). `--period` несовместим с `--since`; `--until` без `--since/--period`, `until` в будущем и `since >= until` — exit 2. Без флагов окна — прежнее поведение (полная история обязательна). Скан newest-first останавливается после первой страницы, где есть tx с `blockTime < since` (провайдер: `HeliusProvider::with_stop_before_block_time`; страниц за граничной не запрашивается) — это complete для окна; исчерпание бюджета страниц/запросов до границы — `incomplete`, exit 3. Tx вне `[since, until)` отбрасываются до леджера; tx без `blockTime` в просмотренном диапазоне — coverage gap (`incomplete`). `until < as_of` по-прежнему требует прохода от новейшего края до `until` (серверный фильтр времени не предполагается). Инвентарь, предшествующий окну, — `LeftCensored` (эпизоды `left_censored`, не оцениваются и не входят в суммы/ratios); `exclude_unknown_basis` не срабатывает на одну левую цензуру. Для `incomplete` кошельков потолок активности (`insider`) проверяется на полностью наблюдаемых днях (все наблюдаемые дни, кроме самого старого) и добавляется причиной после `incomplete_coverage`; кошелек остается исключенным. `run_meta.window = {since, until, since_unix, until_unix, as_of, as_of_unix, source: period|explicit|none}`; в table первой строкой `# window [...)`. В observed-метриках: `left_censored_episodes`, `left_censored_amount_raw`, `has_left_censored_inventory`. Ledger version `solana-wallet-ledger/6` (ADR-015: Jupiter v6 swap-ноги как evidence route swap; `stats.route.route_swaps_by_evidence` = `curve|pump_amm|jupiter|jupiter_only`; `stats.diagnostics.evidence_samples[]` — до 5 образцов malformed/unknown/orphan по кошельку, также на stderr).

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


**ADR-013 (ledger/4, wallet-stats).** Леджер учитывает кошельковые route swaps и единицы котировки SOL/USDC/USDT (лоты несут единицу; один FIFO на `(wallet, mint)`; погашение лотов другой единицы — `Unknown { CrossQuoteUnit }`, эпизод `closed_unknown`; эпизод, реализованный в двух единицах, тоже unknown). JSONL `stats.quote_units[]` — блок на единицу (sol, usdc, usdt): `status`, closed_known, wins/losses/breakeven, `realized_trade_pnl`, `consumed_acquisition_basis` как `{raw, decimal}` (точные десятичные строки: SOL 9 dp, USDC/USDT 6 dp; null без known closed эпизода), `realized_cost_roi` (точные числитель/знаменатель + percent), win_rate, profit_factor, open-episode PnL; суммы разных единиц никогда не складываются. `stats.route` — счетчики `route_swaps`, `route_swaps_by_quote{sol,usdc,usdt}`, `route_leg_not_wallet_price`, `route_rejected_{wallet_not_signer,multi_asset,not_opposite_signs,no_quote_leg,no_verified_leg,passthrough_nonzero}`; `stats.trades.route` — стороны route swaps. Изменения формата: `stats.quote_unit="lamports"` теперь означает единицу прежних SOL-полей (`realized_*`, `profit_factor`, `failed_trade_fees`); `closed_episodes_known/wins/losses/breakeven/win_rate` — по всем единицам; headline `realized_net_pnl` — SOL-блок (`n_a`, если нет known closed SOL эпизодов); `episodes[]` получили `quote_unit`, `pnl_raw`, `pnl_decimal`, `known_disposal_pnl_decimal`, а `pnl`/`pnl_sol_exact`/`known_disposal_pnl` заполнены (не null) только для SOL-эпизодов; `run_meta.ledger_scope` — разрешенные единицы и правило route swap. Таблица: колонки `realized_pnl_usdc`, `realized_pnl_usdt`, `route_swaps`; строка эпизода `pnl_<unit>=`. Фикстуры роутер-кошельков 9oC3/tAwv (по 100 tx): 36/62 route swaps, все USDC; realized USDC −24745.390534 (5 closed, 0W/5L) и −7030.355795 (5 closed, 2W/3L); SOL-блок пуст.
Срез Solana pump.fun bonding-curve + PumpSwap AMM, SOL-леджер в лампортах (ADR-010, ADR-012): один FIFO на `(wallet, mint)` по обеим площадкам; сделка PumpSwap атрибутируется кошельку только при сверке его собственных owner-keyed legs (router-forward не атрибутируется; квота, оплаченная другим аккаунтом, — Unknown basis, не 0); в `stats` счетчики сделок по площадкам/вариантам и диагностика `router_forward_trades_not_attributed`, `quote_funded_elsewhere_trades`, `reversed_pool_trades`; `scope` содержит обе программы и оба IDL-пина; НЕ декодируются Raydium, Meteora, Orca; hop-события Jupiter v6 (`SwapEvent`/`SwapsEvent`, ADR-015) — только evidence swap-ноги для route swap (владение и цена — по собственным дельтам кошелька); маршруты без ноги pump и без события Jupiter (напр. `DF1ow4ts…`) — движения токенов остаются разрывами непрерывности, Unknown, не нулевой PnL; нужен `SCOUT_HELIUS_API_KEY`. EVM/смешанный вход и вход без ключа — exit 4 (кошельки не отбрасываются молча). Флаги: `--input`, `--input-format lines|jsonl`, `--format table|jsonl`, `--detail summary|full`, `--sort input|realized-net-pnl`, `--max-pages-per-wallet N` (1..=200, по умолчанию 10), `--page-limit N` (1..=1000, по умолчанию 500; бюджет страниц остается в страницах, транзакционный бюджет кошелька = `pages × page-limit`; свыше 500 tx на страницу лимит размера ответа поднимается, максимум 64 MiB), `--server-window[=true|false]` (по умолчанию включен; при окне дополнительно `filters.blockTime {gte, lt}`), `--token-accounts none|balance-changed` (по умолчанию `balance-changed`; `filters.tokenAccounts`), статус-фильтр к сканам кошельков НЕ применяется (комиссии неуспешных tx, ADR-004). Все эти опции проверены live 2026-10-03 (`docs/p0/measurements/2026-10-03-helius-filters-live.md`); значения по умолчанию — константы `DEFAULT_PAGE_LIMIT_ARG`, `DEFAULT_SERVER_WINDOW`, `DEFAULT_TOKEN_ACCOUNTS` в `main.rs` каждого бинаря, а `--page-limit 100 --server-window=false --token-accounts none` воспроизводят прежний запрос; эффективные значения — в `run_meta.scan.provider_options` и строке `provider options:` на stderr. `--max-requests N` (N ≥ 1; без флага — без лимита, но счетчик ведется; все HTTP-попытки запуска, ретраи включены; `requests_made` и `max_requests` печатаются в stderr и в `run_meta`). `--period <N>d` / `--since` / `--until` — окно анализа ADR-011 (семантика как в §4: strict RFC 3339 UTC, `[since, until)`, остановка скана на первой странице старше `since`, бюджет до границы — `incomplete`/exit 3, tx вне окна отбрасываются, tx без `blockTime` — gap, неверное окно — exit 2). Карточка и JSONL показывают `left_censored_episodes` (эпизоды, чей инвентарь предшествует окну: считаются, не оцениваются, не входят в суммы), `left_censored_amount_raw`, `has_left_censored_inventory`, `coverage.transactions_in_window`; таблица — колонку `left_censored` и первую строку `# window [...)`; `run_meta.window` и ledger version `solana-wallet-ledger/6` (ADR-015: Jupiter v6 swap-ноги; `metrics.route.route_swaps_evidence_*`, `metrics.route.evidence_samples[]`).

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

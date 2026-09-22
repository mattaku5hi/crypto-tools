# Приемочные сценарии

Все ожидаемые результаты формализуются fixtures/assertions. Нельзя заменять проверку списком «поддерживается». Числа ниже — тестовые данные, не реальные результаты трейдеров.

## A. Идентичность и ввод

**A01.** Один EVM token address разрешается на двух включенных сетях → AMBIGUOUS_CHAIN; явно заданная сеть корректно снимает неоднозначность.

**A02.** Bare EVM wallet без сети → не выбирать сеть по первой найденной активности. `--evm-scope all` создает per-chain identities. Same hex не объединяется в chain-address режиме.

**A03.** Solana mainnet/devnet address bytes совпали → cache/events не смешиваются. Genesis mismatch RPC → отказ до ingestion.

**A04.** Одинаковый symbol у двух token addresses → два AssetKeys. Дубликат входного адреса в другой текстовой форме → одна canonical identity. Invalid address → ошибка с номером строки до live fetch.

**A05.** Lines/CSV/JSONL дают одинаковый список identities. Conflicting per-line chain/CLI chain → ошибка. Missing JSONL footer с run_meta → incomplete upstream, не complete shortlist.

## B. Покупки и пересечения

**B01.** Wallet A купил T1/T2; B купил только T1; C получил T1/T2 airdrop. K=2 → только A.

**B02.** A купил T1 сто раз в разных пулах и T2 один раз → hit_count=2, не 101.

**B03.** A купил T1 и позднее полностью продал → исторический hit остается. Текущие holders не используются как фильтр.

**B04.** Маршрут SOL→USDC→T1: один экономический buy T1; USDC как intermediate с итоговым нулевым net delta не создает hit. Outer aggregator + inner DEX не дублируют сделку.

**B05.** Router.sender или EVM tx.from является relayer/bundler; доказанный owner другой → hit owner. Нет доказательства → ambiguous, не confident hit relayer.

**B06.** Atomic roundtrip T1 с zero end net acquisition не попадает в default buyer-intersect. Ledger при этом сохраняет отдельные распознанные actions и их fees.

**B07.** Liquidity withdrawal, wrap, bridge receipt, reward и обычный transfer не считаются buy. Поддерживаемое приобретение на bonding curve считается buy.

**B08.** K=3 из пяти токенов, один token scan падает → denominator остается 5, report partial. `--match all` не превращается в all-four.

**B09.** USD buy threshold включен, часть buy prices unknown → неизвестные покупки не приравниваются к нулевым; completeness отражает невозможность применить фильтр.

**B10.** Пул мигрировал/закрылся; исторический покупатель обнаруживается через historical registry/index. Отсутствие пула в current list не удаляет историю.

## C. Бухгалтерия

**C01.** Покупка: consideration=1000, fee=10. Продажа: proceeds=1400, fee=10. Expected realized_trade_pnl=380, consumed_basis=1010. Fee sum=20, не 40.

**C02.** Куплено 100 единиц за 1000 + fee10; продано 40 за 600 - fee6. Expected consumed_basis=404, realized_trade_pnl=190; remaining_basis=606. База не списывается целиком при partial sale.

**C03.** Два buy lots по разным ценам, частичные продажи: FIFO allocations совпадают с hand-calculated expected. Измененный completion order RPC не меняет result.

**C04.** Продан токен, приобретенный до report window. Warmup восстанавливает basis; realized transaction относится к окну продажи. Недостаточный warmup → unknown basis, не zero.

**C05.** Входящий transfer с неизвестной покупной ценой затем продан → proceeds известны, total profit не объявляется известным. Строгий ranking исключает material unknown-case. Stats показывает unknown lot и known subset отдельно.

**C06.** Outgoing transfer → inventory уменьшен без realized sale PnL. Explicit self-transfer → lot lineage сохранен. Два неизвестных адреса не связываются по эвристике.

**C07.** Swap не-quote A→B → disposal/acquisition с одной согласованной consideration valuation; combined fee allocations равны фактическому fee. Intermediate hops не создают внешние deposits/withdrawals.

**C08.** Acquisition fee капитализирована в remaining lot, sell fee уменьшает proceeds, failed attempt expense учтен отдельно. Полный fee не вычтен повторно.

**C09.** Solana priority fee уже входит в фактическую total fee. Отдельный подтвержденный tip учитывается один раз. Rent своего token account не назван автоматически невозвратной trading fee.

**C10.** Base fee fixture содержит все применимые исторические components; Robinhood adapter проверяется на собственной семантике. Нельзя просто прибавить L1 fee поверх поля, которое уже ее включает.

**C11.** Gas sponsor оплатил исполнение → расход не приписан кошельку без доказанного reimbursement. Unallocated network expenses видимы и не теряются.

**C12.** Fee-on-transfer получил меньше, чем pool output → ledger использует actual owner flows. Неподдержанная rebasing/Token-2022 extension вызывает explicit uncertainty, а не ложную сверку.

**C13.** Max U256 и необычные decimals → точная сериализация либо checked error; нет wraparound, cast-to-f64 или default decimals=18.

**C14.** Один и тот же batch применен 1, 2, 10 раз → ledger checksum одинаков. Новая decoder projection не суммируется со старой.

## D. Цены и метрики

**D01.** Куплено за SOL, курс SOL/USD при покупке отличается от текущего. Basis использует historical quote conversion. End mark использует отдельный as-of price.

**D02.** Stablecoin price=0.85 USD в fixture → не подставить 1.00. Цена из тонкого sole pool не становится verified без anchor evidence.

**D03.** Есть непроверяемая открытая позиция: displayed market value=N/A; отдельный stress scenario может быть zero-valued только с явно указанной assumption. Unknown value не увеличивает strict quality score/rank.

**D04.** Win rate считается по полностью наблюдаемым episodes, не по числу частичных sells. Breakeven episode входит в denominator и не считается win.

**D05.** Episode opened до окна либо still-open → censored/open cohort counts; не смешивается с default opened-and-closed-in-window cohort.

**D06.** Нет отрицательных episodes: PF status=no_observed_losses, JSON value=null; 0/0 → undefined. No JSON Infinity/NaN/999 surrogate.

**D07.** 7 eligible wallets при `--top 20` → ровно 7. 0 eligible → пустой complete список с exclusion reasons. Все incomplete observations остаются в metadata.

**D08.** Same WalletReport/snapshot в rank и stats → одинаковые PnL, ROI и counts, независимо от formatter.

**D09.** Депозит $1000 без trading activity не дает $1000 period equity PnL. Equity fees не вычитаются вторично. Opening inventory/transfers не обходятся shortcut `realized+current_unrealized`.

**D10.** Неполная equity series или unknown cash flows → portfolio drawdown/Sharpe=N/A. Отдельный drawdown cumulative realized PnL не помечен portfolio drawdown.

**D11.** Размер числовой сортировки не зависит от округления в table. Ties разрешаются deterministically canonical WalletKey. Undefined metrics не попадают в finite order без documented policy.

**D12.** Rank history загружает весь declared wallet trading universe, а не только discovery tokens. `--exclude-assets` явно изменяет scope/manifest и подпись PnL.

## E. Сканирование и надежность

**E01.** EVM adaptive range получает too-many-results → split без потери и дублей. Single-block oversize → explicit alternative/capability gap, не silent partial.

**E02.** Provider JSON-RPC batch возвращает ответы в переставленном порядке и один error → matching by id; retry только error item в пределах quota.

**E03.** Native Solana address history не включает fixture closed token-account incoming transfer → индекс/собственный ownership history восстанавливает его либо explicit coverage gap. Mint-only query никогда не объявляется full token history.

**E04.** Неподдержанная tx version/null getTransaction/unavailable historical state → durable raw/error provenance и gap; никаких silent skips.

**E05.** Одинаковые slot timestamps, разные tx indexes, out-of-order fetch → canonical FIFO result. Unknown tx order не подменяется сортировкой по signature.

**E06.** Reorg удаляет buy и заменяет canonical suffix → rollback/replay пересчитывает buyer hit, basis и ranking. Старый snapshot invalidated.

**E07.** Crash после raw-file publish, до DB manifest; crash после raw checkpoint, до decode; crash после events, до ledger. Resume ведет к одному expected result. Orphan segments безопасно очищаются.

**E08.** Endpoint failover дает другой chain/genesis/head → reject или reconciliation, не mixed ledger. Provider disagreement видим.

**E09.** 429/Retry-After, 5xx, timeout → bounded retries и fairness. Auth/unsupported/schema error не крутится бесконечно. Budget exhausted → checkpoint + partial status.

**E10.** Two CLI processes используют embedded store: no corruption/duplicate canonical events, bounded SQLITE_BUSY retries, consistent immutable snapshots. Не требуется доказать отсутствие всех межпроцессных duplicate HTTP reads.

## F. Highload и SDK

**F01.** Concurrency 1/8/32 → одинаковые normalized ledger/report checksums. Source completion order случайный.

**F02.** Data size растет 1m→10m normalized events при неизменном memory budget. Sorting/aggregation spills; queues не растут бесконечно. Peak RSS измеряется, а не оценивается только по channel counts.

**F03.** Slow storage и slow stdout consumer → backpressure достигает fetch scheduler. Нет unbounded tasks, response buffering или hidden collecting of full history.

**F04.** Один hot wallet составляет значительную часть потока: deterministic ledger, bounded memory, fairness остальных shards; benchmark описывает skew.

**F05.** Frozen complete snapshot + `--offline` → zero network calls. Повторная аналитика может читать raw/normalized cache, но не refetch или переписывать несовместимую semantic projection.

**F06.** CPU-heavy decoding не блокирует Tokio I/O progress. Cancellation завершает bounded work; начатый spawn_blocking не считается magically aborted. stdout BrokenPipe не дает panic/backtrace.

**F07.** Embedding example работает в уже существующем Tokio runtime с injected source/store, без subprocess CLI, global subscriber и process::exit. Scanner-only build не подтягивает обязательную ledger/formatting инфраструктуру.

**F08.** Benchmark intersection >=100k events/s — кандидатный target на определенном hardware для уже нормализованного корпуса, не claimed measurement. Независимо от достижения target отчет содержит actual throughput, p95 latency где применимо, RSS, bytes/event, cache state, source overhead и dataset checksum.

## G. Безопасность и воспроизводимость

**G01.** API key в URL/query/error body не появляется в logs, fixtures или snapshot manifest. Redaction tests включают nested errors.

**G02.** Token symbol с ANSI escape/control characters не исполняет terminal sequences. Огромные/malformed/compressed RPC responses ограничиваются по размерам и не вызывают panic.

**G03.** Изменение decoder/registry/price/cost-basis/fee policy изменяет report fingerprint. Нельзя использовать старый derived cache как новый результат.

**G04.** Discovery и evaluation scopes сохраняются раздельно. Фильтр убыточных токенов не остается невидимым в PnL подписи.

**G05.** Testnet данные не попадают в mainnet денежный рейтинг без explicit request; нет private-key/sign/send-transaction requirement.

## Минимальные property-инварианты

Для известных потоков inventory: opening + acquisitions + incoming - disposals - outgoing - explicit burns = closing. Для costs: acquisition basis распределяется между disposed/transferred/remaining lots без потери и дублирования. Сумма fee allocations + unallocated fee = actual fee. Для idempotency: apply(E); apply(E) = apply(E). Для replay: canonical(snapshot+suffix)=canonical(full history). Для intersections: повторное событие не увеличивает distinct hit_count. Для sorted reports: одинаковый manifest/input дает идентичный deterministic result независимо от concurrency.

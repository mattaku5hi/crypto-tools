# План реализации и задания агенту

Каждый этап завершается кодом/артефактами и проверяемой демонстрацией. Следующие этапы не оправдывают фиктивную поддержку текущего. Ограниченный vertical slice допускается и явно маркируется. Никакие календарные оценки здесь не предполагаются.

## P0. Проверка выполнимости и corpus

**P0.1 — Source capability matrix.** Для каждой из четырех сетей заполнить TokenMarketActivity, WalletActivity, raw transactions/receipts, historical state, native/internal flows, historical token-account ownership, prices, finality, earliest history, paging, limits, billing и provenance. Разделять `documented`, `fixture_verified`, `live_verified`, `unsupported`, `unknown`. Выбрать реальную комбинацию индекс/RPC; не обещать один universal provider. Отдельно проверить, какие queries дорогие и где нужен собственный индекс.

**P0.2 — Deployment registry.** По официальным ABI/IDL/deployment docs собрать небольшой подтвержденный protocol scope. Для Base — один реально развернутый DEX как первый slice; для Solana — один launch/AMM path с buyer attribution; для BSC и Robinhood — свои подтвержденные deployment'ы, без переноса чужих адресов. Указать version/activation/upgrade ranges и pinned source commit.

**P0.3 — Ground-truth corpus.** Минимум 30 hand-checked экономических сценариев суммарно, включая positive/negative examples покупки, partial sells, transfers, route hops и fees. Для каждого: chain/block/tx location, raw fixture, provenance, expected actions/owner/amounts, confidence и rationale. Требуется сочетание synthetic fixtures для крайних случаев и реальных on-chain samples для protocol behavior. Private API keys удалить. Нельзя объявлять synthetic fixture реальной mainnet транзакцией.

**P0.4 — ADR.** Утвердить смысл buyer, trader identity, episode, FIFO, fee allocation, scope/coverage, source strategy и quote-asset policy. Сохранить known gaps и необходимую конфигурацию.

**Выход:** `docs/CAPABILITIES.md`, `config/deployments/*`, `tests/fixtures/*`, ADR-001… и рекомендация source mix с неопределенностью стоимости. Если нет credentials, явно отметить live verification unavailable; не выдумывать результаты. Это не препятствует созданию contracts/mock/offline implementation, но live capability остается неподтвержденной.

**Gate:** по первому slice есть raw→expected economic action проверка; для остальных сетей есть конкретный source plan и честный статус, а не пустые строки.

## P1. Workspace, доменные контракты и input/output

**P1.1.** Создать Cargo workspace по ARCHITECTURE, закрепить совместимый stable toolchain и Cargo.lock. Сначала подключить минимальные features зависимостей.

**P1.2.** Реализовать ChainKey, WalletKey, AssetKey, PoolKey, RawEnvelope, EconomicAction, RawAmount, Money, QualityReport, ReportManifest, typed errors. Реализовать checked serialization и deterministic ordering.

**P1.3.** Реализовать lines/CSV/JSONL adapters, address validation, duplicate handling, network conflict detection, common CLI config. Утилиты пока могут работать с fixture source, но не притворяться live scanners.

**P1.4.** Определить JSON Schema для input/output и versioned metric definitions. Human formatter использует те же WalletReport данные.

**Gate:** fmt/clippy/tests проходят; input→canonical identities→output round-trip; неоднозначный EVM адрес не выбирает случайную сеть; extreme amount values не теряются. Three CLI `--help` описывают реализованные flags; неработающие live modes возвращают explicit Unsupported/ConfigurationRequired.

## P2. Транспорт, планировщик и устойчивое raw storage

**P2.1.** Реализовать reusable HTTP/RPC clients, chain identity preflight, per-provider capability discovery. Вынести transport от EVM/Solana semantics.

**P2.2.** Реализовать bounded admission по count/bytes, quota groups, timeouts, retry classification, Retry-After, jitter, circuit breaker и failover. JSON-RPC batches сопоставлять по id.

**P2.3.** Реализовать SQLite WAL embedded store и compressed raw segments с crash-safe publish. Separate raw/decode/ledger watermarks и snapshot manifests. БД не держит write lock во время сети.

**P2.4.** Mock RPC server с latency/out-of-order/429/5xx/truncation/page overlap/incomplete history. Resume и per-run singleflight. Checkpoint mismatch обнаруживается.

**Gate:** медленный sink не вызывает неограниченного роста памяти; retry quota не превышена; падение между file publish и DB commit не теряет acknowledged данные; одинаковые raw responses не создают duplicate events. Профиль baseline сохранен.

## P3. Первый EVM vertical slice на Base

**P3.1.** Implement EVM adapter: adaptive logs, tx/receipts, optional trace capability, historical metadata, canonical block references и chain fee adapter interface.

**P3.2.** Один подтвержденный DEX decoder и historical pool discovery. Не использовать только current pool list. Pool event sender не считать автоматически wallet.

**P3.3.** Ownership + route normalization + transfer classification. Разобрать минимум direct swap, routed swap, partial sale, transfer, fee-on-transfer unsupported path. Неуверенное отделить.

**P3.4.** Базовый buyer-intersect и базовый ledger на fixtures, end-to-end CLI smoke для ограниченного declared protocol scope.

**Gate:** token list→buyers→wallet fixture history→PnL воспроизводим; raw references доступны; по тем же входам/stored snapshot офлайн получен тот же результат. Net-buy definition не считает intermediate route token купленным активом пользователя.

## P4. Solana vertical slice

**P4.1.** Solana native adapter: transactions/versions/account keys/loaded addresses/meta, token-account owner history semantics, slot/transaction ordering и canonical commitment.

**P4.2.** Indexed WalletActivity adapter с проверкой historical limits/closed accounts; отдельный token/pool discovery source. `getSignaturesForAddress(mint)` не является fallback, претендующим на полноту.

**P4.3.** Первый подтвержденный launch/AMM decoder, затем route normalization и migration path. Native balance reconciliation включает fees и rent/account flows, а не считает все отрицательные lamports стоимостью покупки.

**P4.4.** Golden fixtures: закрытый token account, CPI, split route, fee payer≠owner, unsupported version, failed tx, mint migration, Token-2022 fee semantics либо явный unsupported gap.

**Gate:** одинаковые экономические сценарии Solana и EVM дают одинаковый canonical action contract; ошибки версии/ownership не теряются. Buyer discovery не зависит от текущего списка holders.

## P5. Полная бухгалтерия, pricing и отчеты

**P5.1.** FIFO lots, partial disposals, transfer in/out, self-transfer lineage, unknown basis, opening inventory warmup, exact fee allocation. Property tests conservation/dedup/replay.

**P5.2.** Historical quote/USD pricing, valuation provenance/freshness, stablecoin depeg cases, missing prices, illiquid open positions. Fallback не маскирует questionable price как verified.

**P5.3.** WalletReport и метрики согласно definitions; episode cohort/censoring, PF undefined/infinite statuses, win rate intervals, concentrations. Full equity/drawdown prerequisites отдельны; N/A допустимо до реализации, ложные числа — нет.

**P5.4.** Wallet-rank: default top20, default realized-net-pnl, public quality gates, deterministic ties, exclusions. Wallet-stats: все входные кошельки, N/A/no_activity/partial cards, human and machine formats.

**P5.5.** Direct/staged pipeline, inherited metadata и проверка completion footer; shared cache без повторного получения неизменных данных. Budget/date/quality scopes входят в report fingerprint.

**Gate:** hand-calculated ledger corpus совпадает точно; две утилиты показывают одинаковые values одной метрики для одинакового snapshot. Мало eligible кошельков → короткий топ. Большая бумажная прибыль без надежной цены не улучшает strict rank.

## P6. BSC, Robinhood и расширение DEX scope

**P6.1.** BSC profile: source, chain identity, finality, fees, deployment registry, конкретные Pancake/другие подтвержденные decoders. Проверить логовый RPC, не полагаться на неподходящий публичный endpoint.

**P6.2.** Robinhood profile: current mainnet identity, archive/history capability, fee semantics и существующие DEX versions. Не использовать testnet как доказательство mainnet PnL. Нет нужного wallet index → реализовать честный собственный indexing path либо оставить explicit unsupported query; не выдавать network support за full analytics support.

**P6.3.** Приоритетные дополнительные Solana/EVM decoders по P0 evidence. V4 singleton, Aerodrome/Slipstream и Pancake forks добавляются отдельными semantics tests, не ABI rename.

**P6.4.** Для каждого declared decoder: минимум direct buy, sell, transfer/non-swap negative, routed/contract-wallet и error/upgrade fixture. Не каждый протокол имеет все варианты; неприменимость документируется.

**Gate:** четыре сети проходят публично описанный capability matrix. Можно выполнить реальные три пользовательских сценария в каждой сети внутри поддержанного scope. В README ясно, что не покрыто. Одного рабочего `eth_chainId` недостаточно для сдачи.

## P7. Highload, reliability и контроль ресурсов

**P7.1.** Профилировать реальные bottlenecks по отдельным pipeline stages. Оптимизировать repeated fetch/decode, allocation/copy, lookup locality, queue batching. CPU parsing не блокирует I/O runtime.

**P7.2.** Wallet sharding, bounded canonical reorder, external sort/spill, hot-wallet skew, large-cardinality intersections. Проверить fixed memory budget при росте данных.

**P7.3.** Reorg invalidation + replay; crash injection на всех persistence boundaries; provider divergence; credential/quota failures; interrupted JSONL pipeline; cancellation при active CPU jobs.

**P7.4.** Criterion/controlled performance suite, mock network и cold/warm real-source measurements отдельно. Targets явно отделены от measured values. Добавить regression threshold только после устойчивого baseline.

**Gate:** все сценарии ACCEPTANCE; report invariance при concurrency 1/8/32; bounded resource evidence; raw/ledger checksums после replay совпадают. Нет обещания «lock-free везде», есть подтверждение отсутствия hot global contention.

## P8. SDK, документация и release

**P8.1.** `examples/embedded_scanner.rs`: стороннее Tokio-приложение импортирует scanner, получает stream, отменяет и продолжает; не требуется subprocess CLI. Второй пример подключает normalize/ledger опционально.

**P8.2.** Документировать data sources/capabilities, installation, env setup, costs/limits, missing-data behavior, examples, metric glossary, migrations, storage cleanup и source-of-truth версии.

**P8.3.** CI feature matrix, reproducible fixture tests, dependency/security/license checks, release builds, opt-in live smoke workflow. Ни одного ключа/секрета в git history или logs.

**Gate:** новый пользователь с настроенными источниками может воспроизвести три CLI сценария; downstream проект компилируется с одними scanner features. Все ограничения показаны до принятия решения по рейтингу.

## Что не нужно делать первым

Не начинать с Kubernetes, Kafka, собственного indexer cluster на все блоки четырех сетей, искусственного universal quality score, собственного unsafe lock-free контейнера или десятков пустых protocol adapters. Сначала один проверяемый сквозной slice и общие contracts. Но ограниченный slice не называется законченной поддержкой всех сетей.

# Архитектура trader-scout

Версия 1.0 • 21 сентября 2026 года • проектная спецификация, не реализованный продукт.

## 1. Главный принцип

Три небольших CLI над общим SDK, а не три независимых сканера. Низкоуровневая библиотека чтения сетей ничего не знает о рейтингах и PnL. Аналитические модули ничего не знают о JSON конкретного поставщика или о stdout. Цена, декодирование DEX и идентификация владельца — самостоятельные ответственности.

```text
buyer-intersect      wallet-rank      wallet-stats
          \              |              /
                    scout-app
              /                    \
       scout-analytics          scout-engine
       /      |      \          /     |     \
 ledger   pricing  normalize  scan   storage  providers
                     |       /   \
                  dex-*   solana  evm
                              \    |    /
                               scout-core
```

Стрелки на схеме показывают логические связи, не полный Cargo dependency graph. Точный ациклический граф: core — нижний слой; scan/normalize/ledger/analytics зависят от своих портов и core; реализации provider/storage/DEX подключаются composition root в app/SDK. Инфраструктурные адаптеры не создают обратных зависимостей на CLI.

Scope v1: публичная спотовая DEX-торговля, историческое исследование, локальный read-only инструмент. Первым рабочим slice делаем один подтвержденный EVM DEX на Base, чтобы проверить всю цепочку от RPC до PnL; затем Solana и остальные сетевые профили. Финальная v1 обязана иметь реальные проверенные адаптеры всех четырех заявленных сетей в объявленном protocol scope, а не четыре enum-варианта.

## 2. Сети, адреса и границы идентичности

Под Robinhood принимается **Robinhood Chain**. В документации на дату проверки указаны mainnet chain ID 4663 и testnet 46630 [S01–S02]. Base mainnet — 8453 [S04], BSC — 56 [S03]. Для Solana идентичность кластера проверяется по genesis hash и конфигурации, а не только по текстовому алиасу `mainnet`.

```text
ChainKey  = (family, network_id, genesis_identity)
WalletKey = (ChainKey, address_bytes)
AssetKey  = (ChainKey, Native | Token(address_bytes))
PoolKey   = (ChainKey, protocol, contract_address, optional_pool_id)
```

Одинаковый адрес на Base и BSC — две записи. Одинаковый тикер на разных сетях — не одна монета. Для отчетного объединения допустимы отдельно: `evm-address` grouping по одинаковым байтам и явный `entity-map` пользователя. Они не доказывают общего человека/владельца; особенно это важно для smart accounts. Solana ↔ EVM автоматически не связывать.

Auto-detection имеет ограниченный контракт. Формат адреса позволяет определить семейство; для EVM токена проверяем выбранные сети, контракт на требуемом historical snapshot и token metadata/deployment evidence. Один кандидат → выбрать; несколько → AMBIGUOUS_CHAIN; отсутствие в доступном индексе → NOT_FOUND_OR_UNOBSERVED, а не доказанное отсутствие. Адрес EVM-кошелька сам по себе не кодирует сеть; даже найденная активность в одной сети не доказывает отсутствие другой. Для неуточненного кошелька пользователь выбирает explicit chain либо `--evm-scope all`, получая отдельные записи по сетям.

Если вход утилиты 1 смешивает Solana и EVM, обычные пересечения выполняются в пространстве chain-address. `--match all` по несовместимым пространствам без entity mapping может быть структурно пустым; CLI обязан это объяснить, а не выдумывать связи.

## 3. Предлагаемый workspace

```text
trader-scout/
  Cargo.toml
  Cargo.lock
  rust-toolchain.toml
  bins/
    buyer-intersect/
    wallet-rank/
    wallet-stats/
  crates/
    scout-core/           # domain types, errors, exact amount types
    scout-scan/           # source ports, capabilities, plans, cursors
    scout-rpc/            # transport, quotas, retries, endpoint health
    scout-evm/            # EVM blocks/logs/receipts/traces + chain profiles
    scout-solana/         # Solana RPC/account/transaction semantics
    scout-providers/      # vendor history/discovery adapters
    scout-dex-evm/        # versioned EVM protocol decoders
    scout-dex-solana/     # versioned Solana protocol decoders
    scout-normalize/     # actions, routes, ownership and asset flows
    scout-pricing/       # historical prices, quality, valuation policies
    scout-ledger/        # deterministic inventory, lots, fees, realizations
    scout-analytics/     # intersections, metrics, eligibility and ranking
    scout-storage/       # persistence ports + embedded implementation
    scout-engine/        # orchestration, backpressure, watermarks, replay
    scout-sdk/           # convenient public facade, feature flags
    scout-app/           # input/config/output shared by the three binaries
  config/
    scout.example.toml
    chains/              # validated profiles, finality and fee policies
    deployments/         # program/factory addresses and activation ranges
  schemas/
  tests/fixtures/
  benches/
  examples/embedded_scanner.rs
  docs/adr/
```

Не создавать отдельную копию EVM-движка для BSC/Base/Robinhood. Нужны один EVM engine и три профиля: network identity, finality, fees, provider capabilities, deployment registry. Поддержка нового EVM профиля не означает автоматической поддержки его DEX.

`scout-dex-*` можно сначала держать отдельными внутренними модулями внутри двух crates; не создавать crate на каждую функцию. Отделять пакет, когда он имеет независимый контракт/зависимости или нужен downstream-проекту.

## 4. Публичный API и переиспользование

Пример формы контрактов; конкретные structs агент формализует в P1. BoxFuture/BoxStream здесь позволяют object-safe provider registry без скрытого runtime.

```rust
pub trait HistorySource: Send + Sync {
    fn capabilities(&self) -> SourceCapabilities;

    fn plan<'a>(
        &'a self,
        request: &'a ScanRequest,
    ) -> BoxFuture<'a, Result<ScanPlan, ScanError>>;

    fn scan<'a>(
        &'a self,
        task: ScanTask,
        cancel: CancellationToken,
    ) -> BoxStream<'a, Result<ScanEnvelope, ScanError>>;
}

pub trait TxDecoder: Send + Sync {
    fn decode(
        &self,
        transaction: &RawTransaction,
        context: &DecodeContext,
    ) -> Result<DecodedTransaction, DecodeError>;
}
```

В SDK также существуют порты `DiscoverySource`, `PriceSource`, `RawStore`, `EventStore`, `CheckpointStore`, `FeeModel`, `OwnershipResolver`. Не делать гигантский trait, для которого каждый адаптер обязан имитировать все методы.

`ScanRequest` описывает TokenMarketActivity, WalletActivity или ExplicitBlockRange, период, finality, scope и требуемый quality contract. `SourceCapabilities` описывает доступные запросы, earliest retained point, возможность исторических logs/receipts/state/traces, token-account-aware history, historical ownership, pagination, batch/stream/replay, ограничения размера и provenance. Возможности проверяются по (provider, network, method, time range), а не глобальным `supports_evm=true`.

`ScanEnvelope` передает raw batches, observations о диапазоне и статистику источника. Объявление источником окончания диапазона — не durable checkpoint. Checkpoint создается только после устойчивого сохранения. Поток возвращает bounded chunks, не Vec всей истории.

Downstream-проект должен уметь: импортировать только `scout-evm`/`scout-solana` + `scout-scan` для raw data; добавить `normalize` для economic events; добавить ledger/analytics только при необходимости. SDK не требует установленного CLI, отдельного сервера, другого Tokio runtime или конкретной БД.

## 5. Источники данных и планирование запросов

Три backend-пути: `indexed` — исторические индексы поставщиков; `rpc` — проверяемые блоки/logs/receipts и локальная индексация; `hybrid` — быстрый discovery через индекс с загрузкой/проверкой raw данных и собственными декодерами. Для исследовательского MVP основной режим — hybrid. Собственный глобальный индекс всех сетей — масштабирование после работающего slice, а не обязательный первый шаг.

Разделить две разные задачи:

**Token → buyers.** Нужны исторические рынки/пулы/кривые этого токена, их swaps и атрибуция экономического покупателя. API текущих holders недостаточно. Миграции между bonding curve и AMM, разные DEX/пулы и singleton pool IDs должны входить в discovery.

**Wallet → trading history.** Нужны все релевантные активы и действия кошелька в выбранном scope, а не только монеты, по которым его нашли. Нужны также входящие/исходящие transfers для inventory reconciliation и сетевые расходы, а не один vendor-filter `type=SWAP`.

### EVM

Для token scan: historical pool registry → adaptive `eth_getLogs` по проверенным deployment'ам/PoolId → deduplicated tx hashes → receipts/transactions → traces только при необходимости → decoders/ownership. Transfer logs — дополнительный способ discovery/reconciliation, не классификатор покупок.

Для wallet scan: индекс normal tx + token activity + internal/native flows + protocol events, либо общий block/log/trace scan с локальными wallet indexes. Стандартный JSON-RPC не дает дешевой полной истории произвольного кошелька [S05]. Один фильтр `Transfer` пропускает часть native/contract/account-abstraction активности; `tx.from` не покрывает все smart-wallet действия. Планировщик должен выбирать источник с требуемыми возможностями или вернуть capability gap, а не бесконечно сканировать всю сеть без предупреждения о бюджете.

Различать historical blocks/receipts и archive state. Не каждый исторический запрос требует archive state, и наличие archive state не означает готовый wallet index. `eth_call` для metadata/balance на историческом блоке требует соответствующей state retention.

На перечисленных публичных BSC RPC `eth_getLogs` отключен [S03]; production backend должен пройти preflight для логов и глубины истории. Подключение к endpoint само по себе не является подтверждением пригодности.

### Solana

Raw transactions декодируются с учетом account keys, loaded addresses, поддерживаемой transaction version, inner instructions/CPI, pre/post token balances, historical token-account ownership, native balances и fees. Slot timestamp nullable; отсутствующее время не заменять временем получения ответа.

`getSignaturesForAddress` ищет упоминание адреса в account keys [S06]. Поэтому один mint address не является полным индексом всех swaps/переводов, а один owner pubkey — всех действий его token accounts. Текущий `getTokenAccountsByOwner` не восстанавливает сам по себе уже закрытые аккаунты и прошлую принадлежность. Нужен token/pool index, исторический owner-aware index либо полный block/program scan с локальным индексированием.

Helius расширенная история — кандидат для WalletActivity [S08–S09], но не автоматическая реализация TokenMarketActivity. Исторические ограничения, закрытые/сменившие владельца accounts и ошибки версии обязаны попадать в capability report. Yellowstone — кандидат для tailing; depth replay проверить отдельно, он не заменяет произвольный исторический backfill [S10].

### Матрица DEX

В P0 создать registry: network, protocol family, version, program/factory/manager address, activation block/slot, deactivation/upgrade boundary, source/commit hash, fixtures и coverage status. Никаких выдуманных адресов.

Планируемые семейства: Solana Pump bonding curve/PumpSwap, Raydium AMM/CPMM/CLMM, Orca Whirlpool, далее Meteora; EVM Uniswap-style v2/v3, Uniswap v4, отдельные Aerodrome/Slipstream и Pancake-specific варианты по подтвержденным deployment'ам. Это очередь реализации, **не утверждение, что универсальный v3 decoder корректно обслуживает все форки**. Jupiter/другие routers нормализуют маршрут поверх исполнений DEX, а не создают дополнительную независимую сделку за каждый hop. Для Robinhood проверяется реально развернутый набор протоколов; наличие EVM профиля не заменяет этот этап.

Первый slice поддерживает ограниченный опубликованный список DEX. В отчете всегда выводится этот список. Обновление ABI/IDL не должно молча переписать старую историю: raw data и decoder version сохраняются.

## 6. Канонические данные и качество

```text
RawEnvelope:
  chain, provider_id, request_scope, fetched_at,
  block_hash_or_slot_identity, payload_hash, raw_bytes

EconomicAction:
  action_id, canonical_location, tx_id, operation_path,
  actor_roles, economic_owner, attribution_status,
  protocol, route_id, pool_keys,
  kind, asset_flows[], fees[], raw_references[], decoder_version

TradeView:
  wallet, operation_id, asset_in, amount_in,
  asset_out, amount_out, optional_usd_valuation, fee_allocation
```

`TradeView` — производная проекция для обычного swap. Источник правды — multi-leg EconomicAction: одна транзакция может иметь несколько пользователей/операций/активов. Не загонять все в один `buy_token, sell_token`, теряя сложные операции.

`kind`: Swap, Transfer, Wrap, Unwrap, Bridge, Mint, Burn, Airdrop, Reward, LiquidityAdd, LiquidityRemove, Fee, Unknown. Unknown сохраняет balances/evidence; известная часть транзакции может обрабатываться, но unresolved asset flows нельзя игнорировать в PnL.

Ключ raw event включает chain, block identity, tx id и event/trace/instruction path. Normalized projection имеет decoder version и lineage; новая версия не суммируется со старой в активном ledger. В EVM canonical order — block number, tx index, event/action path. В Solana — slot, block transaction index, instruction/CPI path. Lexicographic signature/hash не заменяет transaction index. При отсутствии порядка fetch block metadata либо статус ORDER_UNKNOWN для чувствительных метрик.

Amounts — unsigned native precision (`U256` для общего контракта, проверяемый conversion из Solana u64). Signed flows хранят sign+magnitude или checked signed big integer. Money/cost basis — точная fixed-point/arbitrary precision арифметика с явным rounding. Decimals неизвестны или превышают поддержанную арифметику → typed error/unknown, не guess=18 и не silent overflow. В JSON большие числа и денежные значения сериализуются строками.

### QualityReport

Не использовать единственный фиктивный `confidence=0.97`. Хранить независимые измерения:

- source scope и declared retention, scanned intervals и gaps;
- `discovery_completeness = complete_within_declared_scope | partial | unknown`;
- decoded/unsupported/failed counts с явно указанным знаменателем;
- attribution classes `verified | inferred | ambiguous` и основания;
- cost-basis coverage для затронутого inventory, включая opening lots;
- execution-price и end-valuation coverage, freshness/liquidity/independence;
- canonical/finality status, reconciliation residuals;
- known/unknown unallocated transaction fees.

100% декодирования полученных транзакций не означает 100% обнаружения транзакций. Если знаменатель неизвестен, percentage=null. Строгие eligibility gates оценивают критические gaps, а не среднее нескольких процентов.

## 7. Что считается покупкой и как искать пересечения

`buyer-intersect` v1 использует **net acquisition по wallet/token в успешной транзакции**: токен действительно поступил экономическому владельцу, есть распознанное исполнение обмена и оплаченная им встречная стоимость, а итоговый net delta этого token по его экономическим счетам положителен. Это предотвращает попадание промежуточных router hops и чистых атомарных roundtrips/flash-loans в обычных покупателей. Бухгалтерский ledger при этом сохраняет реальные отдельные действия внутри транзакции, а не неттирует всю историю.

Transfer receipt, airdrop, rewards, liquidity removal, mint и wrap не дают buy-hit. Поддерживаемая покупка на launch/bonding curve дает hit. Самопереводы между token accounts одного владельца неттируются. Неоднозначный owner не включается в строгий buyer list.

Порог `--min-buy-usd` применяется к подтвержденной покупке. При включенном USD-фильтре отсутствие цены не означает ноль: запись получает PRICE_UNKNOWN и влияет на completeness результата. Без USD-фильтра пересечения могут работать по raw asset flows без dollar pricing.

Алгоритм: каждому distinct входному AssetKey присвоить TokenId. Каждому qualifying `(WalletKey, TokenId)` соответствует один hit независимо от количества buys и пулов. Streaming reducer поддерживает bitmap/small sorted set токенов, counts и first-buy evidence на кошелек. Результат — hit_count>=K, K=2 по умолчанию, либо все N входных токенов. Ошибка сканирования одного токена не уменьшает N.

Память O(число distinct wallet-token memberships), время агрегации ожидаемо O(число qualifying events). Для больших cardinality — sharding и disk spill. Для <=64 токенов достаточно u64 bitmask; иначе dynamic bitset/Roaring после измерений. Полнопарная Jaccard-матрица — опциональный отдельный расчет, не обязательный O(N²) этап.

Одна сеть/пул/транзакция читается один раз на уникальный запрос scope в пределах run, а не заново для каждой монеты и кошелька. Пересечения показывают адрес, сеть, hit_count, tokens, first-buy timestamps/locations, spend при наличии цен, holding status при запросе и quality flags. Это не доказательство инсайдерства или общего реального трейдера.

## 8. Бухгалтерия и PnL

Период отчета по умолчанию — 30 дней, фиксируемый в начале run; пользователь может задать `[since, until)` UTC. **Период отчета и глубина восстановления себестоимости — разные вещи.** Warmup идет до доверенного opening-lot snapshot или доказанного нулевого inventory/первых релевантных приобретений. Лимит backfill, например 365 дней в sample config, — бюджетный предохранитель, не доказательство достаточной истории.

v1 cost basis: FIFO, полное сохранение lots и realized allocations. Смена метода — новая policy version и replay. Для чистой покупки cost basis = actual consideration + выделенные расходы приобретения. Для продажи net proceeds = actual proceeds - выделенные расходы продажи. Частичная продажа расходует соответствующие доли lots; остаток приобретенных расходов остается в открытом lot.

```text
realized_trade_pnl = net_sale_proceeds - consumed_acquisition_basis
realized_net_pnl   = Σ realized_trade_pnl(in report window)
                    - attributable_expensed_trading_overhead(in window)
```

Overhead здесь — только расходы, еще не включенные в cost basis/proceeds: например надежно атрибутированные неудачные торговые попытки. Не считать любые расходы кошелька торговыми автоматически. Все фактические сетевые расходы и нераспределенный остаток показываются отдельно.

Проверочный пример: actual consideration $1000 + acquisition fee $10, sale proceeds $1400 - sale fee $10 → realized_trade_pnl=$380; realized matched-cost ROI=$380/$1010. Нельзя вычесть те же fees второй раз из realized_net_pnl.

Если купленный токен A отдан за другой не-quote токен B: это disposal A и acquisition B с единой согласованной оценкой consideration. Общая fee распределяется по версиям policy так, чтобы сумма allocations равнялась фактической fee; расход не удваивается. Промежуточные route hops не становятся самостоятельными инвестициями кошелька. Для native/stable quote assets действует опубликованный `quote_asset_policy`: их FX PnL не притворяется качеством выбора meme tokens. Full-wallet NAV анализ, напротив, учитывает ценовые изменения всех включенных активов и является отдельной метрикой.

### Transfers и неизвестная себестоимость

Входящий внешний transfer создает inventory с unknown basis, если нет доказанного lot lineage. Нельзя приписать нулевую покупную цену и объявить всю продажу прибылью. Outgoing transfer уменьшает inventory и переносит/выносит basis, но сам по себе не является продажей. Известные self-transfers внутри явно заданной группы сохраняют lot lineage; неизвестные другие адреса не объединяются эвристически. Операции CEX и off-chain приобретения могут остаться неопределенными.

Известная часть realized PnL выводится как **known subset**, а не как total. Кошелек с material unresolved PnL-critical inventory не допускается в строгий общий рейтинг. Нельзя отсортировать только понятные прибыльные сделки и скрыть непонятные.

### Комиссии и специальные токены

Исторический fee adapter выбирается по сети и блоку. Solana учитывает фактическую fee и доказанные отдельные tips; priority fee не добавляется повторно поверх полного fee поля [S14]. Refundable rent/создание собственного token account — не автоматически невозвратная торговая комиссия. Base имеет отдельные fee components [S15]; Robinhood использует свой проверенный fee adapter [S16], а не копию Base. Gasless/sponsored execution отражает, кто реально заплатил.

Fee-on-transfer и Token-2022 fees анализируются по фактическим owner deltas, с сохранением отличий от pool amounts. Rebasing/interest-bearing/неподдержанные token extensions требуют отдельной reconciliation policy; неизвестная семантика запрещает confident PnL. Slippage и DEX fee, уже встроенные в actual settlement amounts, не вычитаются еще раз как гипотетические затраты.

## 9. Исторические цены и открытые позиции

Разделить execution valuation (оценка фактически обмененного consideration в момент операции) и mark-to-market/end valuation (оценка оставшегося inventory). Для покупки за SOL/ETH/BNB нужен курс quote/USD в момент операции, а не текущая цена base token. Stablecoins не считать безусловно $1. Каждая цена содержит timestamp, asset/chain, source, method, confidence class, stale bound и liquidity evidence.

Не принимать цену из того же тонкого манипулируемого пула как независимое подтверждение USD-PnL. Требуются anchor assets/рынки и политика достоверности. Если обе стороны обмена нельзя надежно оценить, USD-PnL остается неопределенным; raw token flows доступны.

Для открытых позиций показывать known remaining basis, amount, marked value, marked unrealized PnL, price freshness и liquidity/sellability flags. Spot valuation не равно реально извлекаемой ликвидности. Отсутствие котировки — N/A, не $0. Отдельный явный stress-сценарий может считать непроверяемый остаток нулем, но он не смешивается с observed value и маркируется assumption. Большие бумажные прибыли тонких рынков не должны поднимать строгий рейтинг.

При полном cash-flow/balance/valuation покрытии можно вычислять дополнительную метрику:

```text
period_equity_pnl = V_end - V_start
                    - external_deposits_valued_at_transfer
                    + external_withdrawals_valued_at_transfer
```

Здесь V — стоимость целиком определенного портфеля на границах окна, внешние потоки исключают swaps и внутренние перемещения. Сетевые fees уже отразились в equity, поэтому повторно их не вычитать. При cross-chain/entity view проверенные внутренние переводы исключаются из external flows; в per-chain view bridge out/in может быть внешним. При неполном universe, непонятных переводах или неполных ценах метрика N/A. Эта формула не заменяется `realized_pnl + current_unrealized`: такой shortcut некорректен для окна с opening inventory и transfers.

## 10. Метрики и рейтинг

Одна функция `analyze_wallet` строит `WalletReport`; rank и stats используют ее результат без разных трактовок.

Основные метрики: realized_net_pnl; known-subset PnL и unknown exposure; realized matched-cost ROI; number of fully observed closed episodes; win rate и его интервал; profit factor; median episode ROI; average/median holding time; traded distinct assets; active trading days; volume; fees and fee burden; concentration of positive PnL in largest token; open inventory/basis/valuation; period equity PnL и drawdown только при достаточных данных; DQ flags.

**Episode** — для wallet/asset inventory от нулевой позиции до следующего нулевого состояния с опубликованным raw-unit dust rule. Partial sells не превращаются в несколько выигранных трейдов. Эпизод с непроверенными transfers/неизвестной себестоимостью не участвует в win rate. Default cohort — полностью наблюдаемые эпизоды, открытые и закрытые внутри отчетного окна; left-censored и still-open counts показываются отдельно. Поэтому сумма episode PnL может не совпадать с realized PnL всех продаж окна — это разные явно названные выборки.

```text
win_rate = positive_closed_episodes / all_valid_closed_episodes
profit_factor = sum(positive episode pnl) / abs(sum(negative episode pnl))
realized_cost_roi = Σ realized_trade_pnl / Σ consumed_acquisition_basis
```

Breakeven episode входит в denominator win rate, но не в positive/negative sums profit factor. Результат net zero определяется точной ledger арифметикой, не округленной таблицей. Overhead распределяется по надежно связанным episode; unallocated amount показывается и не теряется. `realized_cost_roi` — доходность использованной себестоимости, не annualized return, не доходность капитала и не ROI по всему кошельку. Самостоятельные overhead не скрываются: рядом выводить realized_net_pnl и expensed_overhead; при необходимости отдельная явно названная net ratio policy.

При отсутствии отрицательных episodes PF математически unbounded при положительном numerator; сериализация — `{value:null,status:"no_observed_losses"}`. При отсутствии положительных и отрицательных — undefined. Не превращать в число 999 и не сериализовать JSON Infinity. Сортировка PF ставит `no_observed_losses` выше finite PF только для прошедших sample gates и явно показывает статус.

**Default rank:** `realized-net-pnl` descending, затем matched-cost ROI descending, valid episode count descending, canonical WalletKey ascending. Это рейтинг реализованного результата, **не утверждение, что кошелек лучший по полному экономическому результату**. Open losses/unknown valuation показываются обязательно. Дополнительные `--rank-by realized-cost-roi|profit-factor|period-equity-pnl` работают только при наличии prerequisites. Не нормализовать произвольный score в «вероятность прибыли».

Default `quality` profile — предлагаемые настраиваемые исследовательские gates: минимум 20 valid closed episodes, минимум 7 active trading days, discovery complete в объявленном scope, отсутствие material unknown basis/ambiguous flows/неоцененных расходов, необходимые цены на всех ranking-critical операциях, resolved open-exposure valuation для осторожного shortlist. Непроверяемые открытые позиции могут исключать из quality shortlist, но не исчезают из обычного `wallet-stats`. Числа 20/7 — стартовая policy, не статистическая гарантия. `--profile none` убирает sample gates, но не разрешает выдавать неизвестный PnL за известный.

Возврат `--top N` содержит min(N, eligible_count) записей; если eligible_count=0, результат пустой с причинами. Не добивать двадцатку низкокачественными кошельками. Рейтинг включает полную таблицу exclusions в machine-readable metadata. Кошельки, проигнорированные из-за provider errors, не считаются успешно оцененными.

Max drawdown/Sharpe/Sortino не выводить из списка закрытых сделок как портфельные метрики. Для portfolio drawdown нужна consistent time-sampled, flow-adjusted/unitized equity curve. Падение cumulative realized PnL можно отдельно показать в долларах с названием `realized_pnl_drawdown`, не смешивая его с portfolio drawdown. First version может вернуть portfolio drawdown=N/A до реализации всей prerequisites chain.

## 11. Highload-конвейер

```text
input + pinned as-of
 -> plan/discovery
 -> bounded request scheduler
 -> fetch raw batches
 -> durable raw store
 -> bounded CPU decode + normalize
 -> canonical reorder / external sort
 -> wallet-sharded ledger + reducers
 -> immutable reports
 -> bounded output
```

Tokio multi-thread runtime — I/O scheduling. Rust async tasks/futures здесь выполняют роль легковесных кооперативных задач; отдельные OS threads на каждый RPC не нужны. CPU-heavy parsing/ABI decode/sorting/compression выполняются в bounded Rayon pool или коротких bounded blocking jobs. Длительные loops используют dedicated threads с cancellation points; уже запущенный spawn_blocking нельзя считать отмененным простым abort [S18].

**Лимиты и backpressure.** Ограничиваются global и per-provider/per-method in-flight, queued batch count и bytes, response body size, cache bytes, reorder/spill buffers, max retries, waiting jobs и output buffers. Bound на число сообщений без bound на bytes недостаточен. Медленный storage/output останавливает upstream. Не создавать миллионы ожидающих semaphore futures/tasks. Использовать bounded dispatcher, buffer_unordered/JoinSet с ограниченным admission. Tokio bounded channels поддерживают backpressure [S17].

**Transport.** Reuse reqwest/Alloy clients и connection pools [S19–S20]. Отдельные connection/request/body/idle deadlines; HTTP/2 при фактической поддержке. Batch RPC — только при capability support, сопоставление response по id, а не порядку; retries только failed elements. Batch count не равен стоимости по тарифу провайдера.

**Scheduler.** Weighted token buckets по quota group (аккаунт/ключ/метод), adaptive concurrency по latency/error rate, fairness между сетями/кошельками, circuit breaker и bounded jittered retry для transient errors. 429 учитывает Retry-After. Auth/schema/unsupported-method не ретраятся бесконечно. Failover не допускает смешения чужой сети/неподходящей finality/history; проверяется snapshot identity.

**EVM ranges.** Динамическое разбиение по block range, числу logs, response bytes и timeout. Если cap превышен — split; если ответ ровно на известном hard cap, проверить truncation risk. Если даже один block слишком велик — другой capability/source или explicit gap, не silent truncation. Сигнал source completeness остается scoped: нет универсального способа доказать честность любого API только по HTTP 200.

**Lock contention.** По `hash(WalletKey) % shards` события отправляются владельцу состояния. Каждый shard хранит собственные maps/lot queues без глобального hot lock. Atomics подходят для counters; read-mostly registry передается immutable Arc/snapshot. Не обещать «полностью lock-free» для runtime/DB; цель — отсутствие конкурентной записи в общую горячую структуру. DashMap не является lock-free [S22]. Custom unsafe queues — только после профиля и отдельного обоснования.

**Порядок.** Параллельно fetch/decode; sequential wallet reducers получают canonical order через watermarks/reorder buffer. Historical reverse pagination спулируется и сортируется на диске, если объем превышает budget. Нельзя выполнять FIFO в порядке готовности сетевых ответов. Один очень активный кошелек может занимать один shard: profiling должен учитывать hot-key skew; нельзя разделять его ledger по потокам без детерминированного merge.

**Повторная работа.** Per-run singleflight для идентичных запросов, durable caches по chain+snapshot+request/projection version, объединение pool/wallet discoveries до fetch. Cross-process dedup гарантируется в данных; отсутствие всех duplicate RPC между независимыми CLI не обещается без общего coordinator. Static metadata кэшировать по historical validity interval; metadata upgrade может менять interpretation.

Первичная оптимизация — уменьшить число/объем внешних запросов и повторных декодирований. Максимальная конкуренция без учета квот не равна максимальной полезной пропускной способности.

## 12. Хранилище, resume и reorg

Embedded v1: SQLite WAL для manifests, checkpoints, normalized events/indices, materialized ledger references и отчетов; raw payloads в immutable compressed segment files на локальном диске. Parquet — экспорт нормализованных данных и опциональный columnar backend для больших histories, не обязательное место для каждого RPC blob. Не добавлять Redis/Kafka/ClickHouse только ради слова highload. При измеренном превышении single-host возможностей — альтернативный Store backend и persistent ingest service.

SQLite пишет батчами через dedicated writer thread; соединения/очереди ограничены. WAL разрешает concurrent readers, но одновременно один writer [S21]. Несколько CLI processes координируются транзакциями, unique constraints, busy timeout и ограниченными retries; тяжелый ingestion можно сериализовать, не блокируя чтение immutable snapshots. Нельзя держать write transaction на время сетевых запросов. Поддерживается только local filesystem для embedded WAL.

Raw commit protocol: write temp segment → fsync → atomic publish content-addressed file → DB transaction с segment reference и raw coverage checkpoint. Сбой до DB commit оставляет удаляемый orphan, но не потерянный acknowledged raw. Decode watermark обновляется отдельно после durable normalized events; ledger snapshot — после их полного применения. Поэтому существует несколько явных watermarks: fetched, raw_durable, decoded, ledger_applied, finalized. Не выдавать fetched за processed.

Reorg: хранить block hash и parent identity, canonical flag и finality. Переиндексировать затронутый suffix, инвалидировать derived snapshots, восстановить ledger от последнего валидного snapshot и replay. v1 reports по умолчанию используют finalized data; live mode provisional отделен. Provider divergence/deep reorg не заметается под коврик: report invalidated или rebuild-required.

Event uniqueness обеспечивает exactly-once effects при at-least-once доставке. Новая decoder/price/fee policy создает новую projection, а не дублирует старую. Resume plan идентифицируется fingerprint'ом входа, network snapshot, периода и semantic policies; несовместимый checkpoint не продолжать молча.

Общий storage позволяет после `wallet-rank` построить `wallet-stats --offline` без повторной загрузки истории и цен для того же snapshot/policy. `--offline` никогда не делает даже health-check RPC; отсутствующий required data segment дает явную ошибку/partial result. Онлайн повторный run может проверять finality/новые блоки, но не refetch неизменные raw данные без причины.

## 13. Наблюдаемость, стоимость и безопасность

Structured tracing: run_id, chain, provider, method, task_id, block range, latency, retry reason, bytes, cache outcome, decoder version. Метрики: QPS, logical/HTTP requests, provider units, p50/p95/p99, 429/timeouts, pipeline queue bytes, RSS, CPU, decoded/normalized events per second, DB write latency, cache hit/miss, unknown actions, attribution ambiguity, reconciliation gaps и lag watermarks. Все progress/logs в stderr.

`--dry-run` строит план и доступную оценку объема/requests/credits с lower/upper bounds и unknown components. Оценка не притворяется точной. `--max-requests`, `--max-provider-units`, `--max-backfill-days`, `--memory-budget-mib` и deadline ограничивают выполнение; денежный hard cap требует надежной billing model либо provider-side budget, а не только подсчета HTTP calls.

В репозитории только env placeholders. Полные endpoint URLs с keys/query secrets не логировать. Токены/символы/metadata не могут включать управляющие terminal sequences в human output. Есть response/body/decompression/input size limits, typed malformed-data errors, TLS verification и отсутствие signing/send-transaction API в публичном read-only фасаде.

## 14. Проверка производительности

Сначала корректность, затем измеренная оптимизация. Отдельно тестировать raw fetch, decode/normalize, ordered ledger replay, intersection reducer, storage и CLI output. Записывать compiler/toolchain, CPU/cores/RAM/disk, release flags, fixture checksum, event/wallet/token counts, ordering, cache state, network latency и provider limits.

Предлагаемая benchmark-площадка: 8 физических CPU cores, 16 GiB RAM, локальный SSD; pipeline memory budget 2 GiB. Это не обещанная конфигурация пользователя. Кандидатный target только для in-memory intersection reducer: >=100k **уже нормализованных** qualifying events/s на корпусе 1m events/100k wallets/256 assets. Этот target не относится к RPC, JSON decode, PnL, disk или end-to-end и подлежит подтверждению/пересмотру в ADR после baseline.

Обязательные свойства важнее произвольного TPS: bounded RSS при десятикратном росте входа с disk spill; стабильность при медленном consumer; точный результат при random completion order; replay после crash; разумная устойчивость к 429; повторный offline run без сети; правильная работа с hot wallet. В CI не сравнивать абсолютную скорость shared runner с workstation. Регрессии мерить на одной контролируемой машине, допуск согласовать после baseline.

## 15. Дальнейшее использование для трейдинга

Отбор кошельков по списку выросших монет — условная выборка, а не unbiased доказательство навыка. Хранить discovery dataset/period отдельно от evaluation dataset/period; для проверки смотреть другие токены или последующий holdout. Не использовать будущую цену/ликвидность при оценке исторического решения. Не показывать quality score как прогноз.

Копируемость — отдельный будущий модуль: holding time, trade size относительно liquidity, observed latency, concentration, unusual funding и вероятный MEV/atomic style. Флаги являются evidence-based heuristics, не обвинением и не гарантией profit. Приоритет v1 — проверяемая история и правильная бухгалтерия, на которых такой модуль можно построить.

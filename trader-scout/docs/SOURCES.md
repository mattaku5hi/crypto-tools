# Источники и проверенные ограничения

Проверены 21 сентября 2026 года. Конкретные API, лимиты, версии библиотек и адреса deployment'ов агент обязан повторно проверить при реализации. Ниже приведены первичные источники. Архитектурные решения, пороги качества и benchmark-цели в остальных документах — проектные предложения, а не утверждения этих источников.

- **S01 — Robinhood Chain, подключение.** `https://docs.robinhood.com/chain/connecting/` — документация указывает mainnet chain ID 4663, testnet 46630, EVM/Arbitrum-инфраструктуру и отдельные endpoints. Публичные endpoints не рекомендуются для production.
- **S02 — Robinhood Chain, обзор.** `https://docs.robinhood.com/chain/` — Ethereum-compatible L2; наличие сети не означает доступность внутренней брокерской истории Robinhood. Перечень инфраструктуры/DEX на странице не заменяет проверку deployment'ов.
- **S03 — BSC JSON-RPC.** `https://docs.bnbchain.org/bnb-smart-chain/developers/json_rpc/json-rpc-endpoint/` — mainnet ID 56; `eth_getLogs` отключен на перечисленных публичных mainnet-endpoints; finality имеет особенности BSC.
- **S04 — Base, подключение.** `https://docs.base.org/get-started/connect-to-base` — mainnet ID 8453, EVM-совместимость.
- **S05 — Ethereum JSON-RPC.** `https://ethereum.org/developers/docs/apis/json-rpc/` — методы блоков, receipts, logs и block tags. Универсального метода полной истории кошелька в базовом интерфейсе нет; wallet-history требует индекса или более широкого сканирования.
- **S06 — Solana getSignaturesForAddress.** `https://solana.com/docs/rpc/http/getsignaturesforaddress` — поиск по упоминанию адреса в account keys, а не универсальный индекс всех сделок mint/владельца.
- **S07 — Solana getTransaction.** `https://solana.com/docs/rpc/http/gettransaction` — поддержка версий, inner instructions, метаданные и nullable-ответы. Неподдержанная версия не должна исчезать из учета.
- **S08 — Helius, расширенная история адреса.** `https://www.helius.dev/docs/rpc/gettransactionsforaddress` — token-account-aware история и исторические ограничения фильтров. История адреса сама по себе не является API всех покупателей mint. Нужно отдельно проверить поддержку закрытых/сменивших владельца token accounts и заявленную глубину.
- **S09 — Helius Enhanced Transactions.** `https://www.helius.dev/docs/api-reference/enhanced-transactions/gettransactionsbyaddress` — кандидат для индексированной/нормализованной истории; формат поставщика не является внутренней моделью проекта.
- **S10 — Yellowstone gRPC.** `https://github.com/rpcpool/yellowstone-grpc` — источник Solana-streaming. Глубина replay зависит от конкретного сервиса; streaming не заменяет произвольный historical backfill.
- **S11 — Uniswap v3 Swap event.** `https://raw.githubusercontent.com/Uniswap/v3-core/main/contracts/interfaces/pool/IUniswapV3PoolEvents.sol` — sender/recipient и pool balance deltas не являются сами по себе универсальной идентификацией трейдера.
- **S12 — Uniswap v4.** `https://developers.uniswap.org/docs/protocols/v4/overview` — singleton PoolManager; идентификатор пула должен поддерживать PoolId, а не только адрес отдельного контракта.
- **S13 — Pump, официальные документы и IDL.** `https://github.com/pump-fun/pump-public-docs` — источник для версии декодеров и проверки миграций.
- **S14 — Solana fees.** `https://solana.com/docs/core/fees` — base/priority fees. Для исторической бухгалтерии используются фактически уплаченные комиссии из метаданных.
- **S15 — Base fees.** `https://docs.base.org/specifications/transactions/network-fees` — L2 execution и L1 fee components; исторический fee adapter должен учитывать действовавший режим и избегать повторного вычитания.
- **S16 — Robinhood fees.** `https://docs.robinhood.com/chain/gas-and-fees/` — отдельная проверка модели Arbitrum-совместимой сети, не слепое копирование Base-формулы.
- **S17 — Tokio channels.** `https://tokio.rs/tokio/tutorial/channels` — bounded queues, backpressure и владение состоянием через message passing.
- **S18 — Tokio spawn_blocking.** `https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html` — CPU concurrency требуется ограничивать; уже начатая blocking-задача не отменяется обычным abort.
- **S19 — reqwest Client.** `https://docs.rs/reqwest/latest/reqwest/struct.Client.html` — переиспользование клиента и connection pool.
- **S20 — Alloy.** `https://alloy.rs/introduction/getting-started/` — EVM RPC, primitives и ABI, кандидат для EVM-адаптера.
- **S21 — SQLite WAL.** `https://www.sqlite.org/wal.html` — параллельные читатели и один писатель; локальный embedded-backend, а не бесконечно масштабируемая многописательная БД.
- **S22 — DashMap.** `https://docs.rs/dashmap/latest/dashmap/struct.DashMap.html` — concurrent map с locking-поведением; не называть ее lock-free.

## Границы выводов

Полнота считается только относительно явно заданных сетей, периода, протоколов, классов операций и возможностей источника. Документация не доказывает отсутствие пропущенных операций у конкретного провайдера. Пустой ответ, успешно завершенная пагинация или отсутствие кода контракта на latest не являются универсальным доказательством отсутствия исторической активности.

Документы не содержат проверенных live API credentials, измеренной скорости будущей утилиты или полного списка актуальных DEX deployment'ов. Это результаты этапа P0 и последующих benchmark'ов, а не подставляемые догадки.

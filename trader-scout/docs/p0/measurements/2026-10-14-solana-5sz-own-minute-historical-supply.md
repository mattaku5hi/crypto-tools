# B8.4: 5SZ — собственные минуты сделок и историческая эмиссия

14 октября guarded запись добавила условную FDV оценку для
`5SZJnFiptXoXsnsqvUretGomCanVBUHkTfLsmUELpump`:
**$49 041,24–$49 096,36**. В БД записана нижняя граница,
source `onchain-pool-window-supply-cex-reference-v1`, `ath_at=NULL`.
Это максимум среди наблюдавшихся состояний выбранного пула, не подтверждённый
годовой ATH по всем рынкам. Политика: ADR-021, amendment 52.

## Что позволило продолжить проверку

В ±3-second окне Solana Tracker пика 23:06:13 UTC выбранный пул показал только
fee/creator-configuration transaction, а оба vault — пустые страницы.
Из этого не следует, что провайдерская цена ошибочна или реальной цены нет.
Три ограниченных запроса последней истории пула и vault вернули десять swap
транзакций и двадцать PRE/POST состояний, 21:59:26–23:03:48 UTC 19 марта.

Первичное сравнение переносило последний ratio на более позднюю FX минуту;
оно осталось только диагностикой. Для записи каждый из двадцати ratio получил
курс своей фактической минуты из одного Coinbase ответа с 70 candles.
Root и независимый audit проверили raw balances, индексы счетов, mint/owner,
scaling 6/9 и точные FX fractions. Все состояния сохранены; глобальные low/high
максимумы отличаются от provider price примерно на −0,152%/−0,040%.
По всем адресам остались older cursors: полнота ценового окна не доказана.
Внутренняя цена протокола и virtual reserve semantics остаются unknown.

## Историческая эмиссия

Born supply — 2e15 raw units, поздняя current supply — 1694540195901908;
15,27% диапазон слишком широк для подстановки current supply.
Terminal signature census: 756 unique signatures, 604 success и 152 failed.
Latest-price anchor был исключён `before=anchor`; после его добавления получены
605 ожидаемых successful signatures. TFA: шесть страниц по 100, одна из 5,
пустая terminal страница. Множества совпали без пропусков, лишних записей или
дубликатов; replay использует canonical slot/transaction-index order.

Независимо проверены все 718 target-token references: InitializeMint,
два MintTo по 1e15, отзыв mint authority, четыре Burn и 710 non-supply операций.
Unknown/malformed target instructions нет. Producer v1 ошибочно проверял
account 0 для Burn; он сохранён и не использован. Corrected v2 проверяет
account 1, actual raw bytes и parent/inner locators.

Три ранних burn оставили 1703382837434545 raw; крошечный burn 7 raw между
третьей и четвёртой наблюдавшимися сделками оставил 1703382837434538.
Внутри price transactions supply operations нет. Все двадцать состояний
получили эмиссию на своё время. Поздняя current supply ниже end supply на
8842641532630 raw; она не подставлялась в исторические значения.
Current raw account: exact index 17, Token2022, decimals 6, authorities None,
412 bytes, metadata TLVs 18/64 и 19/174 bytes.

## Проверки и фиксация

Golden использует точную U512 арифметику, проверяет ordered supply operations,
Burn account position, изменение 7 raw между состояниями, собственные FX минуты,
все FDV fractions, source/certificate и ограничения модели. High-floor
4909635 cents — диагностика; outward high-ceil **4909636** — верхняя граница.
Возможная ошибка неизвестной модели вне этого numerical FX interval.

Round 25: `cargo fmt --check`, workspace Clippy all targets/features с
`-D warnings`, workspace tests all features — успешно, **1241 tests passed**,
62,89 s, завершено 10:24:41 UTC. `scout_test`, production tests false,
`CARGO_INCREMENTAL=0`. Guard сверил provider price/timestamp/pool/hash и
missing-row version. Первая версия helper остановилась до SQL из-за нового
компактного формата fixture; v2 сравнивает parsed root certificate и выполнила
запись. Holder/liquidity metadata сохранены; runner gates и production fallback
не менялись.

- Producer supply v2: `0943f4cafb94c1c23bcf0f160a0946c381e5bb686b5cdc1a5ceefb640c0c1312`.
- Own-minute price: `d247088bac40da20b0318fcf971f9d2c26c26a00ac918512d061998143618ee8`.
- Independent ledger v3: `bd65e057cb4f466bf654eded5163182cea544631c22f398c13e468cbfe21b17d`.
- Independent price v4: `d853ff336c904fcb762c5ede3d0463be776d9bb73cfde2a410ff3c07ae56cbf6`.
- Root: `1f49e69010899a0d282f41d0e6fb850a03e968fbed650f5c3e9339b60af6c9c3`.
- Golden: `e78a6db01ef229b5794b2039e70336864a9ce08ce983f637f5bd0d95f406c57f`.

Все raw/proof/write manifests — приватные `.scout-data/diagnostics/`.
Расход запросов: prior 8 + supply phase 9 = **17 Helius methods**, отдельный
own-minute Coinbase request — 1. Five methods из cap 22 не использованы.
Это число API methods, не утверждение о billing credits или стоимости.

Контрольная выборка: **523/679 numeric, 156 unknown**, 89 новых значений
относительно 434 baseline: SOL 75 / Base 2 / Robinhood 12. Годовая полнота
не подтверждена этими числами. Sealed 80 package предшествует девяти записям;
полный пакет на каждую оценку не создаём. Следующий task ID: B8.4, оставшиеся
historical price/model/supply gaps; параллельно продолжается контроль демона.

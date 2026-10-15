# Solana Tracker: 100 сохранённых ценовых наблюдений

Проход завершён 2026-10-11 в 15:11:08 UTC: 97 новых HTTP-запросов успешны,
три прежних ответа импортированы из проверенного raw-кеша. В PostgreSQL
схемы 10 сохранены 100 наблюдений `solana-tracker-ath-v1`: у всех есть
положительная точная USD-цена, millisecond timestamp и pool ID.

`ath_price_observations` отделена от `ath_price_windows` и денежной `ath`.
Новый проход не записывал FDV и не менял runner thresholds. Числовой
audit остаётся 425/679 закрытых и 254 unknown; наличие цены ещё не
доказывает максимум исторического FDV при меняющемся supply. Scope
поставщика — его записанная история с неизвестной нижней границей,
не доказанное покрытие lifetime или всех рынков.

## Квота и сохранность

Владелец указал 2 500 запросов в месяц для своего аккаунта. Отдельная
доля приложения ограничена 250: семь ранее известных попыток плюс
97 новых дают 104 учтённых запроса. Внешнее использование аккаунта
неизвестно, поэтому оставшуюся общую квоту не вычисляем. Резервирование
происходит в БД до HTTP, ключи не задают идентичность quota group,
автоматические повторы и redirects выключены. Последовательный
проход ограничен одним запросом в секунду и 2 MiB на тело.

Ответы сохранены до parsing с SHA-256 и key-absence scan. Private
manifest и 97 raw ответов: `.scout-data/diagnostics/solana-tracker-remaining-100-2026-10-11/`.
`combined-observations-100.json` содержит также три старых ответа;
публичные fixture copies имеют только три ранее сохранённых ответа.
Демон остаётся на прежнем работающем бинарнике и не перезапускался.

## Проверки и следующая работа

`cargo fmt --check`, workspace Clippy с `-D warnings`, 1 207 offline
тестов и `cargo build --release -p dev-tracker --locked` прошли.
Изменения SDK/CLI/DB зафиксированы в `be442ee`. Exact-value tests
сохраняют null как null, а schema tests проверяют quota reservation
и отсутствие побочного переноса provider observations в ATH.

Для всех 100 mints отдельно проверены creation receipts: 49 начальных
выпусков по 1B и 51 по 2B, decimals=6, mint authority отозвана после
последнего выпуска. Текущие account extensions не разрешают допечатку.
Исторический FDV нужно сопоставлять с supply во времени. Следующий
шаг B8.4 — проверка точных FDV bounds и независимых USD-котировок,
а также восстановление косвенных Zora quote routes на Base.

Полный schema10 snapshot сохранён в
`.scout-data/snapshots/devtracker-2026-10-11T152507Z-schema10-source.dump`
(2 840 876 439 bytes; SHA-256 `4d822a82df72c917c70c89922077cb7ae7f2831c3ca8c1a342a23be0531536a5`).
Изолированное восстановление прошло: все 100 точных наблюдений совпали,
квота 7+97 сохранена, схема 1–10, 28 292 134 launch rows, 194 522 migrations
и 941 cursor. Audit 425/254 и три отсутствующих ATH строки сохранены.
Контейнер/volume проверки удалены; production database не восстанавливалась.

В `b21db97` добавлена точная арифметика raw-supply bounds и golden cases;
весь workspace прошёл fmt/Clippy и 1 209 tests, release build `--locked`
также прошёл. У двух источниковых наблюдений границы совпадают в центах,
но независимая проверка цены пока не завершена: 8Cc Birdeye high расходится
на 72%, PEC получил 429, исторические pool events не дают проверенного
virtual-reserve spot. В `ath` они не перенесены. Подробнее:
[cent-bound price check](2026-10-11-solana-cent-bound-price-check.md).

## Последующая проверка и две денежные записи

После этого source-only прохода в 17:12 UTC отдельно подтверждены 8Cc и PEC.
Стартовые/ближайшие после миграции PumpSwap состояния и минутный Coinbase
SOL/USD согласуются с vendor price в пределах 1%; mint supply bounds
округляются к одной сумме cents. Guarded missing-only записи — $47 952.52
и $65 651.14, `ath_at=NULL`, provider window/market scope остаются unknown.
Сохранены holders и liquidity. Числовой frozen audit теперь 427/679,
252 unknown. Новых ST/Codex calls на этом этапе не было.

Golden/evidence/ADR amendment19 зафиксированы в54472c5; перед записью
прошли workspace fmt, Clippy и1210 tests. Предыдущий schema10 snapshot
и созданный из него private bundle предшествуют этим двум записям.
[8Cc proof](2026-10-11-solana-8cc-migration-pool-spot-corroboration.md),
[PEC proof](2026-10-11-solana-pec-migration-spot-corroboration.md).

## Ещё две записи и актуальное переносимое состояние

В 17:45:59 UTC отдельно перенесены 5b2 ($121 337.50) и 7DG ($59 125.28).
Полная проверенная burn history доказывает исходный supply в момент
максимальной цены; монотонное уменьшение supply и одинаковый rounded-cent
результат на нижней/верхней границе подтверждают maximum FDV в сообщённом
поставщиком окне. Ближайшие PumpSwap состояния сверены с vault balances,
комиссиями и Coinbase SOL/USD; все концы точных price/FX интервалов
проходят 1%. Источник имеет отдельную historical-supply provenance,
`ath_at=NULL`, provider window/market scope остаются неизвестными.
У 7DG порядок события относительно provider milliseconds неизвестен.
У обеих записей сохранены 4 holders и прежняя liquidity; ни одна из четырёх
новых записей не проходит действующие runner gates.

Frozen audit на 18:03 UTC: 429/679 числовых, 250 unknown; SOL — 14 числовых
и 96 unknown. Новых ST/Codex calls для этих двух записей не было.
Golden cases/ADR amendments 20–21 и evidence закреплены в `7bd0635`;
workspace fmt, Clippy и 1 212 tests прошли. Все четыре release CLI собраны
с `--locked` и проверены в Bookworm runtime.
[Historical supply](2026-10-11-solana-low-burn-peak-supply-history.md),
[price corroboration](2026-10-11-solana-five-low-burn-price-corroboration.md).

Новый snapshot на 17:51:08 UTC включает все четыре guarded FDV-записи.
Полное изолированное восстановление проверило их точные значения и
provenance, все 100 наблюдений, quota 7+97, schema 1–10 и 941 cursor.
Из проверенного dump собран новый private bundle со свежими 36 экспортами;
старые snapshot/archive сохранены как отдельно датированные копии.
Пути, hashes и ограничения: [portable validation](2026-10-11-portable-deployment-validation.md).

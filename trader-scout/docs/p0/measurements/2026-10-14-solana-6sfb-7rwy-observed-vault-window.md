# B8.4: 6SFB и 7rWY — наблюдавшиеся состояния, неизвестные wire arguments

14 октября две условные оценки FDV записаны одной guarded транзакцией.
Источник — `onchain-pool-window-bounded-supply-cex-reference-v1`.
Проверка относится к сохранённым состояниям выбранных пулов, а не к годовому
ATH по всем рынкам. Политика: ADR-021, amendment 51.

| Токен | Успешные / failed транзакции | PRE/POST состояния | Нижняя / верхняя FDV | Supply/FX диапазон |
|---|---:|---:|---:|---:|
| `6SFBnNJvyjvueA1vW7Do3zkHhiKGpDxagsDZavo5pump` | 25 / 14 | 50 | $83 726,84 / $83 800,93 | ~0,0884% |
| `7rWYdrHd8BtZqYPsszusXTzs2MMCPWKxrXyVM6f6pump` | 21 / 2 | 42 | $114 189,86 / $114 357,50 | ~0,1466% |

150 и 126 low/close/high вычислений — по три валютных endpoint на каждое
состояние, а не дополнительные состояния или транзакции. Все raw balances
перепроверены по индексам счетов, mint и принадлежности обоих vault пулу.
Root отдельно сверил каждое отношение резервов и исходный минутный Coinbase
FX с точной арифметикой. Все глобальные наблюдавшиеся low/high/close максимумы
попадают в 2% от сохранённых Solana Tracker пиков; отклонения low/high —
около −0,079%/+0,0093% и −0,121%/+0,0220% соответственно.

## Эмиссия и ограничения

У обоих токенов raw birth receipt подтверждает выпуск 1e15 raw units и отзыв
mint authority в той же транзакции. Поздние mint accounts: exact request
indexes 1/15, slot 456610930, legacy SPL Token, 82 bytes, decimals 6, отсутствуют
mint/freeze authorities и extensions. Эмиссия 999999485135843 / 999961149869882
raw units ограничивает историческую снизу; первоначальный выпуск — сверху.
Точное время сжиганий не восстановлено. Нижняя FDV использует current supply,
верхняя — birth supply; cents округлены наружу.

Сохраняются 18/29 instruction-layout variances из датированного IDL audit.
Недостающие или отличающиеся аргументы не получают фиктивного bool/default.
Отношение реальных vault резервов — условная модель; её равенство внутренней
цене протокола не доказано. Ошибка модели вне указанных supply/FX интервалов.

У 6SFB successful query закончился пустой страницей. У 7rWY initial capture
содержит 21 successful и 2 failed записи; продолжение из 20 successful записей
пересекается с initial capture, затем пришла пустая страница. Это не независимое
доказательство исчерпывающей пагинации окна. Верхняя FDV и максимум относятся
только к наблюдавшимся состояниям; ненаблюдавшийся максимум остаётся unknown.
16 failed транзакций сохранены без price/ledger effects. Unknown ABI, virtual
reserve semantics, другие рынки и годовая полнота не объявлены закрытыми.

## Проверки и запись

Независимый audit проверил raw birth/current mint bytes, pool account offsets,
все состояния и FX. Root проверил повторные строки продолжения на совпадение
транзакции и денежных метаданных, пересчитал 276 endpoint calculations и
92 наблюдения. Golden использует точную U512 арифметику, проверяет глобальные
ценовые отклонения, FDV округление и ограничения source/certificate.

Round 24: `cargo fmt --check`, workspace Clippy со всеми features/targets и
`-D warnings`, workspace tests со всеми features — успешно, **1240 tests passed**,
61,27 s; завершено 09:55:23 UTC. Тестовая БД `scout_test`, production tests false,
`CARGO_INCREMENTAL=0`. Atomic write проверил неизменные provider observation
price/timestamp/pool/hash и missing-row versions. Даты ATH остались null,
holder/liquidity metadata сохранены; runner gates и production fallback не менялись.

- Producer audit v3: `5bc108f193ac465447e3935f4abcb68a6a958310fa8ab5e37be2f589d3f158ac`.
- Independent audit v5: `770c96b2a13a6fc6c131b013d7b54ce308d9e412d9b24476dc2d6abf2e1d5243`.
- Root certificate: `b9394a5fd978140d755f42fdc8571af62d458f1a4b3d419a112b9e7ec3b31bc2`.
- Golden: `b9a2259993bcf1ed7ecc6d224b5feed94c94a372591c50de8b865c963410c456`.

Raw/proof/write manifests — приватные `.scout-data/diagnostics/`.
Контрольная выборка: **522/679 numeric, 157 unknown**; 88 новых значений
относительно 434 baseline, SOL 74 / Base 2 / Robinhood 12. Не переносить эти
числа на годовое покрытие всей сети. Sealed 80 package предшествует восьми
записям; новый полный пакет на каждую оценку не создаём. Следующий task ID:
B8.4, оставшиеся historical price/model/supply gaps и контроль демона.

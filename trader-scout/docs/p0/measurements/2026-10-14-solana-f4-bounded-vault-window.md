# B8.4: F4 — ограниченное ценовое окно и диапазон эмиссии

14 октября в БД записана одна условная оценка FDV для
`F4esbK8e3f6nY5J5McN4JSc5ZCurimUsVGNaQBZApump`.
Нижняя оценка — **$43 871,06**, верхняя — **$43 971,26**.
Это наблюдавшийся максимум отношения реальных резервов выбранного пула
в окне провайдерского пика, а не подтверждённый годовой ATH по всем рынкам.
Политика описана в ADR-021, amendment 50.

## Данные и вычисление

- 61 успешная транзакция, 122 состояния PRE/POST; следующая страница пуста.
  Проверены реальные индексы счетов, mint, принадлежность обоих vault пулу,
  raw amounts и decimals 6/9. Запрос по mint не доказывает полноту других рынков.
- Каждое отношение quote/base пересчитано точно с минутным Coinbase SOL/USD.
  Глобальные нижний и верхний ценовые максимумы отличаются от сохранённого
  Solana Tracker пика примерно на −0,032% и +0,157%; оба проходят порог 2%.
- Raw birth receipt подтверждает выпуск 1e15 raw units и отзыв mint authority
  в той же транзакции: MintTo outer 4/inner 12, отзыв outer 4/inner 13.
- Поздний mint snapshot: индекс запроса 79, slot 455686275,
  999605614638751 raw units, decimals 6, отсутствующие mint/freeze authorities,
  только metadata TLV 18/64 bytes и 19/179 bytes.
- Историческая эмиссия ограничена текущей снизу и первоначальной сверху.
  Точное время сжиганий не установлено. Нижняя FDV — максимум low price ×
  current supply; верхняя — максимум high price × birth supply. Округление
  наружу даёт 4387106–4397126 cents; ширина около 0,23%.

Все 61 события имеют неизвестный 16-byte tail. Нулевые байты не означают
известный тип поля или нулевые virtual reserves. January 9 IDL не описывает
этот хвост, а February 17 cashback IDL опубликован после February 13 событий.
Полная поддержка decoder и равенство отношения vault резервов внутренней
цене протокола не заявляются. Возможная ошибка модели вне диапазона 0,23%.

## Проверки и запись

Независимые raw/model и supply audits прошли; root отдельно сверил все
122 raw balance/FX состояния и FDV границы. Golden regression использует
точную U512 арифметику и сохраняет ограничения модели и неизвестного хвоста.
Источник: `onchain-pool-window-bounded-supply-cex-reference-v1`, как у QBH.

Round 23 выявил unchecked cast в новом тесте; журнал сохранён.
После исправления round 23b: `cargo fmt --check`, workspace Clippy со всеми
features/targets и `-D warnings`, workspace tests со всеми features — успешно,
**1239 tests passed**, 56,99 s. БД тестов `scout_test`, production tests false;
`CARGO_INCREMENTAL=0`. Проверки завершились 09:42:26 UTC.

Guarded SQL проверил исходное missing-row version и сохранённые provider
price/timestamp/pool/hash перед записью. `ath_at=NULL`, holders 25 и liquidity
164200 cents сохранены. Runner gates и production fallback не менялись.
Значение ниже runner threshold; нового включения в список не заявляем.

## Контрольные суммы и продолжение

- Root certificate: `59ca8c53df1fe04f8c198a2ed72f7f899479eadd177496950b98fbe9e514a7c3`.
- Supply proof: `bc0817656ddd75111102c1f0f06b5624516ce986adee9c30bde90ff60854d81b`.
- Independent raw/model: `4400fe289254568bb41357f329e51dd54dec1566734728772069bc59da55fb16`.
- Independent supply v3: `33d8e99b4aefab3f6140db6005108b72a2cd10b538ff2a4ff9376efcaeaa522c`.
- Golden v1: `9eee53346212cfbf3b646b6b7eab892d4c19bc96fcf129915dd35a3d984479e5`.

Приватные raw/proof/write manifests находятся в `.scout-data/diagnostics/`.
После записи: **520/679 numeric, 159 unknown**, 86 новых значений относительно
434 baseline: SOL 72 / Base 2 / Robinhood 12. Это фиксированная диагностическая
выборка; числовое значение само по себе не подтверждает годовую полноту.
Sealed 80 package предшествует шести последующим записям; новый полный пакет
на каждое значение не создаём. Следующий task ID: B8.4, оставшиеся price/model/
supply gaps; параллельно продолжается контроль фонового демона.

Дополнительная проверка 5SZ: в точном окне выбранный пул вернул только
fee/migration transaction, оба vault — пустые terminal pages. Ценовых состояний
не найдено; запись остаётся unknown. Использовано 5 из 8 разрешённых Helius
methods, включая сохранённый malformed request; новых price-provider calls нет.

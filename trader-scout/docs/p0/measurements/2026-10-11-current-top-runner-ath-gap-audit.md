# Проверка ATH-пробелов текущих top-runners — 2026-10-11

## Итог

В ограниченном аудите найдено 150 launch rows без `ath` у разработчиков,
присутствующих в текущих экспортированных top-runners. Это инвентарь
отсутствующих записей, а не 150 текущих задач ATH-очереди. Проверка точного
предиката `DevDb::ath_candidates` показывает, что в очередь сейчас попадают
только **8 из 150**: шесть Base/Zora и два Solana/Pump с записью migration.
Остальные 142 сейчас не выбираются этим предикатом: 111 EVM signer-alias
Flap/FourMeme launches без migration и 31 недавний немигрировавший Pump launch.

У всех 150 наблюдений `holders=NULL`, поэтому ни один пока не проходит runner
holder gate. Восемь выбранных ATH-кандидатов остаются неизвестными по цене и
держателям, но даже они не меняют runner counts до появления корректных ATH и
`holders >= 100`. Отдельно этот аудит не доказывает наличие/отсутствие новых
creator groups вне текущего набора разработчиков.

## Метод и пределы когорты

`DevDb::for_each_dev` нормализует `flap`, `fourmeme` и `pons`: если `creator`
является контрактом без owner, dev становится `launches.signer`; без signer
запуск исключается. См. `SIGNER_RESOLVED_LAUNCHPADS` и реализацию в
`crates/scout-devdb/src/lib.rs:173,780-815`. `DevDb::ath_candidates` выбирает
launch при наличии migration row либо для launchpad из константы `NO_CURVE_LAUNCHPADS` (`zora`, `clanker`, `doppler`,
`bankr`, `noice`, `flaunch`) при достижении creator-порога в три launches; дополнительно выбираются stale ATH rows. В этой выборке все
`ath` rows отсутствуют. См. `crates/scout-devdb/src/lib.rs:159,1066-1100`. Отдельно config списка
`launchpads_without_migration = ["zora", "clanker"]` задаёт top-runners
metric gates, но это не ATH candidate predicate. Top-runners требуют ATH FDV от $500k до $10bn и `holders >= 100` по
`crates/scout-devtracker/src/lib.rs:341-366`.

Frozen679 snapshot сохранён в `2026-10-11 06:33:21Z`; он использовал прямое
сопоставление `launches.creator` и уже отмечал alias limitation. Последние
экспорты top-runners сохранены в `19:40:38 MSK` (`16:40:38Z`): все 71 адреса
из них присутствуют в seed720. DB audit выполнен на `2026-10-11T17:05:47Z`.

| Источник отсутствующих строк | Всего | Без migration | Текущие `ath_candidates` | Сейчас не выбираются |
|---|---:|---:|---:|---:|
| EVM signer aliases: Flap/FourMeme | 111 | 111 | 0 | 111 |
| После freeze: Base/Zora | 6 | 6 | 6 | 0 |
| После freeze: Solana/Pump | 33 | 31 | 2 | 31 |
| **Всего** | **150** | **148** | **8** | **142** |

Для 31 Pump launch без migration возраст меньше настроенного `pending_window_days = 3` на момент аудита; они не включены в ATH selection. Два других Pump token уже имеют migration row. Base Zora launches входят в no-curve selection из-за creator threshold. Flap/FourMeme alias rows не входят: они не мигрировали, а эти launchpads не являются текущими исключениями no-curve ATH selection.

## Alias-строки у текущих BSC/RH разработчиков

Read-only join по годовым `flap/fourmeme/pons` launches, где контрактный
creator без owner разрешается к текущему signer/dev, дал 140 строк:

| Сеть | Alias launches | Без ATH row | ATH rows с holders и FDV $500k–$10bn | Числовой ATH ниже $500k |
|---|---:|---:|---:|---:|
| BSC | 93 | 66 | 7 | 20 |
| Robinhood Chain | 47 | 45 | 0 | 2 |
| **Всего** | **140** | **111** | **7** | **22** |

Все 111 отсутствующих alias ATH rows имеют `holders=NULL`, но они также не
выбираются текущим `ath_candidates` predicate: у них нет migration row, а
Flap/FourMeme не входят в список no-curve launchpads. Этот результат указывает
на различие между alias-aware developer attribution и ATH queue eligibility;
он не доказывает, что их ATH следует добавлять без отдельного изменения
selection policy. Из 29 числовых alias ATH rows семь имеют holders >=100 и
ATH в runner band; ещё 22 ниже $500k. Расклад числовых низких ATH: BSC 20,
Robinhood 2; ни одна из 111 absent строк не является `codex-missing` sentinel
в этой выборке.

| Сеть / dev | Текущий export label | Alias launches | Без ATH row (не выбраны сейчас) | Из alias ATH: runner-qualified / known low |
|---|---|---:|---:|---:|
| BSC `0x3c33…a656f` | `TR m3/44 s0 r3` | 44 | 39 | 3 / 2 |
| BSC `0xc98a…3de9` | `TR m9/11 s0 r3` | 6 | 1 | 0 / 5 |
| BSC `0xde1e…1dab` | `TR m11/269 s0 r3` | 4 | 3 | 0 / 1 |
| BSC `0xe881…7525` | `TR m83/1306 s0 r5` | 30 | 23 | 1 / 6 |
| Robinhood `0x7b44…edf2` | `TR m257/3646 s0 r4` | 26 | 26 | 0 / 0 |
| Robinhood `0xa60e…3065` | `TR m167/3484 s0 r14` | 13 | 11 | 0 / 2 |
| Robinhood `0xe565…ea47` | `TR m13/84 s0 r4` | 8 | 8 | 0 / 0 |

Labels summarize full developer history; the alias subgroup alone does not
recompute those exports or ranks.

В числовом своде также учтены две BSC-группы без отсутствующих ATH rows: dev
`0x1348…12d6` имеет 8 alias rows (3 runner-qualified, 5 ниже $500k), а dev
`0x1bff…191a` — одну числовую low row. Ещё один numeric low alias относится к
этим малым/без-gap строкам; поэтому полные суммы по сети (27 BSC numeric)
выше сумм только семи dev-групп с отсутствующими строками.

## Приоритет и пропускная способность ATH polling

`ath_candidates(chain, now, limit)` сначала сортирует отсутствующие ATH по
`created_at DESC, token`; только после них идут устаревшие наблюдения. Вызов
`observe_ath` ограничивает `limit = requests × 200`, а daemon запускает один
запрос на сеть раз в 240 минут (`config/dev-tracker.toml`). Поэтому при полном
расписании это до 6 запросов на сеть в сутки, 24 запроса в сутки на четыре
сети или 720 за 30 дней; максимальная пропускная способность — 36 000 токенов
на сеть за 30 дней при каждом полном batch. Это оценка расписания, не гарантия
фактических попыток или provider billing. Комментарий в config указывает
10 000 бесплатных Codex requests/month; расписание 720 requests/30d заметно
ниже этой указанной квоты. Но пропускная способность всего Base backlog не
установлена: полный queue count не завершился в пределах SQL timeout, и
остаток исторических eligible tokens неизвестен.

По точному missing-candidate предикату Base/Zora занимают позиции 85, 92,
152, 178, 193 и 205; два migrated Solana/Pump — позиции 29 и 57. Ранг 205
означает, что один Base-токен сейчас оказывается за пределом 200-token batch;
остальные семь находятся в первом batch своей сети. Ранги посчитаны по всем
eligible missing rows, созданным не раньше самого старого целевого токена;
старые записи не могут опережать его при сортировке по убыванию времени. В
этих ограниченных временных окнах находится 205 Base и 57 Solana candidates;
rank snapshot прочитан в `2026-10-11 17:49:31Z`. Перед следующим due-pass более
новые launches могут сдвинуть позиции, поэтому это checkpoint, а не SLA.

Позиции определяются recency, не повышенным приоритетом current runner-dev:
`observe_ath` вызывает `ath_candidates` только с chain/limit, а `watch_set`
применяется к derivation/export, не ATH selection. Для гарантии попадания всех
runner-linked eligible gaps в один batch минимальным изменением порядка было бы
пропускать такие уже eligible токены перед общей recency-очередью, не меняя
membership predicate. Сейчас подтверждён только один токен за лимитом на пять
позиций; без требования same-pass SLA изменение порядка не обязательно. Если
верхние 200 кандидатов будут успешно записаны, оставшиеся missing rows выйдут
в начало следующей очереди; при неуспешном запросе повторяется тот же batch.

Попытка полного rank/count была отменена по 45-секундному statement timeout и
использовала прежний неполный no-curve список, поэтому не служит доказательством
полного backlog. Full-six исторический backlog здесь не подсчитан; окна 205/57
нельзя трактовать как total queue. Точный bounded rank query с шестью
`NO_CURVE_LAUNCHPADS` сохранён в `.scout-data/diagnostics/current-top-runner-ath-gap-audit-2026-10-11/queue-rank-recent.sql`.
Из сохраненного `daemon_state` daemon запущен в 08:04Z и жив на момент audit
17:24Z. В коде `next_ath` хранится только в памяти: первое polling pass due
после первичного ingestion, далее следующий due назначается после полного
цикла + 240 минут. При быстрой обработке это дает ориентировочные проходы около
08:04, 12:04, 16:04 и 20:04Z; это cadence estimate, не журнал фактических
запросов. Точный следующий due и факт конкретной последней per-chain попытки
из БД не восстанавливаются. У launches есть chain creation time, но нет
времени их первого попадания в индекс; поэтому отсутствующая ATH observation
сама по себе не доказывает пропущенный deadline. Для новых восьми ранги
показывают, что отдельный priority override не нужен, чтобы попасть в
ближайший проход.

Read-only фиксированный Codex counter `quota:codex:remaining` показывал 581
на `updated_at=16:13:40Z` (значение не является provider-wide usage). При
24 попытках в сутки такой локальный остаток равен примерно 24 дням полного
расписания, поэтому текущий application allowance может закончиться раньше
следующих 30 дней. Counter не сбрасывается автоматически, а другие Codex
backfills могут тратить тот же allowance.

## После frozen679 и оставшиеся пробелы

По 71 текущему developer key запрос для launch rows, созданных после времени
freeze, вернул 45 строк: 39 без ATH и 6 с числовым Codex ATH ниже $500k.
Отсутствующие строки: Base 6 и Solana 33. Из них только Base 6 Zora и 2 уже
migrated Solana Pump входят в текущий `ath_candidates` выбор; остальные 31 Pump
пока немигрировавшие и младше трехдневного окна.

Все 150 объединённых адресов отсутствуют в таблице `ath` и имеют null holders;
пересечение токенов с frozen679 равно нулю. Private sidecar содержит bounded
candidate list, SQL и TSV, включая предикаты migration/ATH eligibility.

Запрос по всей годовой таблице launches для поиска creator groups вне текущих
экспортов был отменён statement timeout 45 секунд. Поэтому вывод ограничен
разработчиками текущих экспортов и этими launch rows; он не оценивает новые
группы, которые не представлены в seed720 или текущих exports. Экспорты были
сохранены в 16:40Z, SQL snapshot прочитан в 17:05Z, то есть это не атомарный
live snapshot.

## Evidence

Private read-only evidence и повторяемые запросы: `.scout-data/diagnostics/current-top-runner-ath-gap-audit-2026-10-11/`.
`eligibility-scope-query.sql`/`eligibility-scope.tsv` проверяют migration row,
no-curve creator threshold, ATH selection и трехдневный pending cutoff для
всех 150 строк. Остальные SQL/TSV и `summary-and-candidates.json` сохраняют
ограниченные запросы и входные адреса. Изменений БД/кода и provider calls не
было.

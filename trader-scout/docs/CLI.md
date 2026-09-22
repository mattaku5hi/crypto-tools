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

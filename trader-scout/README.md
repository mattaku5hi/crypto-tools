# trader-scout — спецификация и реализация

Версия дизайна: 1.0. Дата проверки внешней документации: 21 сентября 2026 года.

Репозиторий содержит Rust workspace с CLI анализа кошельков и сервисом `dev-tracker`. Архитектурные документы включают целевой дизайн; реализованные пути и оставшиеся ограничения нужно сверять с кодом и `docs/TICKETS.md`.

Проект: три CLI (`buyer-intersect`, `wallet-rank`, `wallet-stats`) над одним переиспользуемым Rust SDK для чтения блокчейнов и анализа публичной спотовой DEX-торговли в Solana, BSC, Base и Robinhood Chain.

## Порядок чтения

Для знакомства с работающим проектом начните с [вводного путеводителя для C++ разработчика](docs/PROJECT-GUIDE.md): архитектура, поток данных, технологии и устройство хранения.

Текущие результаты филлинга, таблицы PnL и проверенный снимок — [фиксация 11 октября](docs/p0/measurements/2026-10-11-devtracker-results.md); проверка бесплатных ATH API — [исследование провайдеров](docs/p0/measurements/2026-10-11-free-ath-providers.md).

1. `AGENTS.md` — обязательные правила реализации.
2. `docs/ARCHITECTURE.md` — границы модулей, сканирование, данные, PnL и highload.
3. `docs/CLI.md` — входы, выходы, опции и примеры.
4. `docs/ROADMAP.md` — последовательность задач и критерии приемки.
5. `docs/ACCEPTANCE.md` — проверяемые сценарии корректности и производительности.
6. `docs/SOURCES.md` — первичная документация и границы подтвержденных фактов.
7. `config/scout.example.toml` — предлагаемый контракт конфигурации; его загрузчик еще требуется реализовать.
8. `docs/rust/` — почему код написан именно так: границы крейтов, типизированная корректность, детерминизм без float/HashMap, async/backpressure и почему не lock-free, дисциплина замеров производительности, тестовая пирамида, требования к библиотеке. Явно помечено, что реализовано и протестировано, что только спроектировано, что не измерено.

## Первая задача агенту

Прочитай все перечисленные документы. Выполни этап P0 из `docs/ROADMAP.md`: составь подтвержденную матрицу возможностей источников, историю/глубину покрытия, deployment registry и референсные транзакции. Затем реализуй P1. Не заявляй поддержку сети, истории кошелька или DEX только на основании успешного RPC-запроса. Не изменяй определения PnL, качества данных и торговой операции без отдельного ADR и тестов.

## Не входит в scope

Исполнение сделок, приватные ключи, подписывание транзакций, скрытая брокерская/CEX-история, идентификация реальных людей, автоматическое доказательство общего владельца разных адресов, прогноз будущей прибыли. NFT, перпетуалы, LP-доходность, кредитование и staking требуют отдельных аналитических модулей: в первой версии такие операции обнаруживаются и маркируются, но не притворяются обычными спотовыми сделками.

## Linux release artifacts

Workflow `trader-scout release` creates an x86_64 Linux package for
`dev-tracker`, `buyer-intersect`, `wallet-rank`, and `wallet-stats`. It runs
workspace formatting, Clippy, and tests with a local PostgreSQL test service,
secret/dependency checks, then builds with Rust 1.98.1 on Debian Bookworm and
smoke-checks the packaged CLIs on the Bookworm runtime image.

Run it manually from Actions to create a reviewable artifact without
publishing a release. Pushing a project-scoped tag such as
`trader-scout/v1.2.3` runs the same gates and publishes the tarball,
`SHA256SUMS`, and JSON manifest as a GitHub Release. The manifest records the
source commit, tag, target, toolchain, lockfile hash, dependency-license
inventory hash, and each binary hash. The archive includes the dependency
license inventory from Cargo metadata; the advisory scan and inventory are
presence/provenance checks, not legal approval of license compatibility.
The package targets `x86_64-unknown-linux-gnu`; it is not a Windows, macOS, or
ARM build. Live provider credentials are not required by this workflow.
The workflow has not yet been run on GitHub. Local packaging checks used the
workspace's existing release binaries; they do not attest a release build of
the current commit.

For a portable host directory and separate verified state bundle, see
[portable deployment](deploy/portable/README.md). The release archive now
contains the runtime template and verifier; database/config/export state and
the separately supplied mode-0600 `deploy/.env` stay outside the public
release.

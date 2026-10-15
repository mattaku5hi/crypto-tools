# Robinhood: проверка трёх кандидатов без ценовой истории

Read-only pilot выбрал три самых ранних Robinhood Doppler-токена из исходной
679-token выборки, у которых ATH неизвестен и holders отсутствуют. Это
`0xa892de026150e67f66a8eb7cdd61ac8a04551e18`,
`0x21ca201ddd11e3b146a0b6a1bfbb0addf9b2843f` и
`0x32cd34b3a7ccb1c7522820b7e1bcf0e0e1e01e18`; все созданы 24 июля 2026.
Birdeye price windows у этих адресов не содержат числового максимума.

Три keyless GeckoTerminal token-pool GET вернули HTTP200 и пустые списки,
без следующей страницы. Три DexPaprika token-latest GET вернули metadata
с совпадающим адресом, но `summary` и `price_stats` равны NULL. Текущего
публичного ценового пути эти запросы не нашли. Пустой индекс не доказывает
отсутствие исторических сделок, а NULL не является нулевой ценой или FDV.

Всего выполнены шесть запросов без автоматических повторов: GT с интервалом
не менее15 секунд, DexPaprika — не менее2.2 секунд; тела ограничены2MiB.
Первое DexPaprika body сохранено до ошибки разбора nullable `price_stats`;
его HTTP status/headers не сохранились. После исправления разбора повторного
запроса не было: использовано исходное body с SHA-256, metadata gap отмечен.
Два остальных HTTP status —200. Ценовых/ATH записей в БД не выполнялось;
chain RPC, Birdeye и Codex не вызывались.

Raw bodies, SHA-256, выборка и request manifests:
`.scout-data/diagnostics/robinhood-empty-price-pool-pilot-2026-10-11/`.
В аудите потенциальных раннеров holders=NULL означает необходимость проверки,
а не доказанное прохождение порога100 держателей. Это pilot трёх адресов,
не проверка всех30 оставшихся Robinhood-кандидатов с неизвестными holders.

Endpoints: [GeckoTerminal token pools](https://api.geckoterminal.com/api/v2/networks/robinhood/tokens/0xa892de026150e67f66a8eb7cdd61ac8a04551e18/pools),
[DexPaprika token latest](https://api.dexpaprika.com/networks/robinhood/tokens/0xa892de026150e67f66a8eb7cdd61ac8a04551e18).

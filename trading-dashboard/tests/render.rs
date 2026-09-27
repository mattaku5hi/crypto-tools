use trading_dashboard::{Fact, Health, Snapshot, render_document, render_status_panel};

#[test]
fn unknown_evidence_is_not_fabricated_as_zero_or_healthy() {
    let facts = [Fact {
        label: "Forecast",
        value: None,
        note: "No model has been trained",
    }];
    let snapshot = Snapshot {
        product: "Polyazimuth BTC 5m/15m",
        mode: "fixture only",
        health: Health::Unknown,
        health_reason: "No real recorder or predictor is running",
        as_of: None,
        facts: &facts,
    };
    let html = render_document(&snapshot);
    assert!(html.contains("Forecast</dt><dd>UNKNOWN"));
    assert!(html.contains("td-state unknown\">UNKNOWN"));
    assert!(html.contains("As of: UNKNOWN"));
    assert!(!html.contains("<button"));
    assert!(!html.contains("profit"));
}

#[test]
fn every_host_value_is_escaped_in_panel_and_document() {
    let facts = [Fact {
        label: "<img src=x onerror=alert(1)>",
        value: Some("<script>bad()</script>"),
        note: "\"quoted\" & 'single'",
    }];
    let snapshot = Snapshot {
        product: "<title>host</title>",
        mode: "<live>",
        health: Health::Blocked,
        health_reason: "<iframe src=x>",
        as_of: Some("<now>"),
        facts: &facts,
    };
    for html in [render_document(&snapshot), render_status_panel(&snapshot)] {
        assert!(!html.contains("<script>"));
        assert!(!html.contains("<iframe"));
        assert!(!html.contains("<img"));
        assert!(html.contains("&lt;script&gt;bad()&lt;/script&gt;"));
        assert!(html.contains("&quot;quoted&quot; &amp; &#39;single&#39;"));
        assert!(html.contains("&lt;live&gt;"));
    }
}

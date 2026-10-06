const CATALOGUE: &[(&str, &str)] = &[
    (
        "Optional Group fair share: --fair-share cpu-request-time|memory-request-time|off. Child Queue/Group weight: --share-weight N (1..10000). Accounts retained reservations over elapsed time; no kernel weight changes.",
        "Optionales Fair Share für Groups: --fair-share cpu-request-time|memory-request-time|off. Gewicht der untergeordneten Queue/Group: --share-weight N (1..10000). Rechnet gehaltene Reservierungen über die verstrichene Zeit ab; keine Änderung von Kernel-Gewichten.",
    ),
    ("fair share", "Fair Share"),
    ("share weight", "Anteilsgewicht"),
    ("normalized service", "gewichtete Ressourcennutzungszeit"),
    ("service watermark", "Mindeststand der Abrechnung"),
    ("active lookahead", "Vorausschau für aktive Reservierungen"),
    (
        "invalid fair-share accounting",
        "ungültige Fair-Share-Abrechnung",
    ),
    (
        "fair-share accounting overflow; admission stopped",
        "Fair-Share-Abrechnung übergelaufen; Zulassung gestoppt",
    ),
    (
        "fair share requires a positive resource request",
        "Fair Share erfordert eine positive Ressourcenanforderung",
    ),
    (
        "share-weight must be an integer from 1 through 10000",
        "share-weight muss eine ganze Zahl von 1 bis 10000 sein",
    ),
    (
        "fair-share requires off, cpu-request-time or memory-request-time",
        "fair-share erfordert off, cpu-request-time oder memory-request-time",
    ),
    (
        "fair-share scopes require a Group",
        "Fair-Share-Bereiche erfordern eine Group",
    ),
    (
        "fair-share basis changes require an inactive subtree",
        "Änderungen der Fair-Share-Abrechnungsbasis erfordern einen Teilbaum ohne aktive Jobs",
    ),
    ("ordering enabled", "Reihenfolgerichtlinie aktiv"),
    ("aging interval", "Aging-Intervall"),
    (
        "ancestor priority bounds",
        "übergeordnete Prioritätsgrenzen",
    ),
    ("backfill", "Backfill"),
    (
        "predicted start (Unix milliseconds)",
        "vorhergesagter Start (Unix-Millisekunden)",
    ),
    ("unknown", "unbekannt"),
    ("base priority", "Basispriorität"),
    ("effective priority", "wirksame Priorität"),
    ("eligible waiting time", "anrechenbare Wartezeit"),
    ("source", "Quelle"),
    ("unset", "nicht gesetzt"),
    ("current policy", "aktuelle Richtlinie"),
    ("launch policy", "Richtlinie beim Start"),
    (
        "Optional admission policy: --priority N (-1000..1000). Collection settings: --priority-min N, --priority-max N, --aging DURATION, --strict-fifo true|false, --backfill off|conservative.",
        "Optionale Zulassungsrichtlinie: --priority N (-1000..1000). Queue-/Group-Einstellungen: --priority-min N, --priority-max N, --aging DAUER, --strict-fifo true|false, --backfill off|conservative.",
    ),
    (
        "priority must be an integer from -1000 through 1000",
        "Priorität muss eine ganze Zahl von -1000 bis 1000 sein",
    ),
    (
        "aging requires a positive whole-millisecond duration within u64 range",
        "Aging erfordert eine positive Dauer in ganzen Millisekunden innerhalb des u64-Bereichs",
    ),
    (
        "strict-fifo requires true or false",
        "strict-fifo erfordert true oder false",
    ),
    (
        "backfill requires off or conservative",
        "backfill erfordert off oder conservative",
    ),
    (
        "priority is outside ancestor bounds",
        "Priorität liegt außerhalb der Grenzen einer übergeordneten Gruppe oder Queue",
    ),
    (
        "ancestor priority bounds do not overlap",
        "Prioritätsgrenzen übergeordneter Gruppen oder Queues überschneiden sich nicht",
    ),
    ("invalid scheduling ledger", "ungültiges Scheduling-Journal"),
    (
        "waiting credit overflow; admission stopped",
        "Wartezeitguthaben übergelaufen; Zulassung gestoppt",
    ),
    ("strict FIFO waits for Job", "strenges FIFO wartet auf Job"),
    (
        "protected opportunity for Job",
        "geschützte Startmöglichkeit für Job",
    ),
    (
        "waiting for resources or an unknown running-job end",
        "wartet auf Ressourcen oder das unbekannte Ende eines laufenden Jobs",
    ),
    (
        "only held or queued Jobs can be reprioritized",
        "nur zurückgehaltene oder wartende Jobs können eine neue Priorität erhalten",
    ),
    (
        "attempt changed before reprioritization",
        "Versuch hat sich vor der Prioritätsänderung geändert",
    ),
    ("no such Job", "kein solcher Job"),
    (
        "usage: job reprioritize ID --priority N",
        "Aufruf: job reprioritize ID --priority N",
    ),
    (
        "usage: job explain ID [--json]",
        "Aufruf: job explain ID [--json]",
    ),
    ("Admission ordering", "Zulassungsreihenfolge"),
    (
        "Priority and aging do not change kernel CPU or I/O weights. Predictions are advisory; indefinite running work or persistent holds can prevent a start.",
        "Priorität und Aging ändern keine CPU- oder I/O-Gewichte im Kernel. Vorhersagen sind unverbindlich; unbegrenzt laufende Arbeit oder dauerhafte Sperren können den Start verhindern.",
    ),
];

pub fn help() -> String {
    ["usage: job reprioritize ID --priority N", "usage: job explain ID [--json]",
        "Optional admission policy: --priority N (-1000..1000). Collection settings: --priority-min N, --priority-max N, --aging DURATION, --strict-fifo true|false, --backfill off|conservative.",
        "Optional Group fair share: --fair-share cpu-request-time|memory-request-time|off. Child Queue/Group weight: --share-weight N (1..10000). Accounts retained reservations over elapsed time; no kernel weight changes.",
        "Priority and aging do not change kernel CPU or I/O weights. Predictions are advisory; indefinite running work or persistent holds can prevent a start."].map(message).join("\n")
}

pub fn message(key: &str) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    if locale.starts_with("de") {
        CATALOGUE
            .iter()
            .find(|(english, _)| *english == key)
            .map_or(key, |(_, german)| *german)
            .to_owned()
    } else {
        key.to_owned()
    }
}

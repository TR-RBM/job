const CATALOGUE: &[(&str, &str)] = &[
    (
        "cgroup pressure accounting is disabled or invalid",
        "cgroup-Druckmessung ist deaktiviert oder ungültig",
    ),
    (
        "pressure trace exceeds the size limit",
        "Druckverlauf überschreitet die Größenbegrenzung",
    ),
    ("invalid pressure trace", "ungültiger Druckverlauf"),
    (
        "pressure trace time must increase within a boot",
        "Zeit im Druckverlauf muss innerhalb eines Systemstarts ansteigen",
    ),
    (
        "pressure trace revisits an earlier boot",
        "Druckverlauf kehrt zu einem früheren Systemstart zurück",
    ),
    (
        "usage: job pressure replay FILE [--json]",
        "Aufruf: job pressure replay DATEI [--json]",
    ),
    (
        "invalid pressure monitor source",
        "ungültige Druckmonitorquelle",
    ),
    (
        "pressure monitor identity changed",
        "Identität der Druckmonitorquelle geändert",
    ),
    (
        "invalid pressure monitor filesystem",
        "ungültiges Dateisystem der Druckmonitorquelle",
    ),
    (
        "incomplete pressure monitor registration",
        "Druckmonitorregistrierung unvollständig",
    ),
    (
        "pressure monitor source closed or polling failed",
        "Druckmonitorquelle geschlossen oder Abfrage fehlgeschlagen",
    ),
    ("invalid pressure rules", "ungültige Druckregeln"),
    ("invalid pressure ledger", "ungültiger Druckzustand"),
    (
        "pressure event counter exhausted",
        "Druckereigniszähler ausgeschöpft",
    ),
    ("pressure admission hold", "Startpause durch Druckregel"),
    ("temporary max-running", "vorübergehendes max-running"),
    (
        "reconsider boot milliseconds",
        "nächste Prüfung in Boot-Millisekunden",
    ),
    (
        "pressure accounting unavailable",
        "Druckzustand nicht speicherbar",
    ),
    ("not sampled", "noch nicht gemessen"),
    (
        "host pressure measurement unavailable",
        "Host-Druckmessung nicht verfügbar",
    ),
    ("pressure policy", "Druckregel"),
    (
        "usage: job queue|group create|set PATH --pressure FILE; unset PATH pressure",
        "Aufruf: job queue|group create|set PFAD --pressure DATEI; unset PFAD pressure",
    ),
    (
        "usage: job pressure status|events [--json]",
        "Aufruf: job pressure status|events [--json]",
    ),
    (
        "usage: job pressure [ID] [--json]",
        "Aufruf: job pressure [ID] [--json]",
    ),
    (
        "usage: job pressure parse FILE --resource cpu|memory|io [--host]",
        "Aufruf: job pressure parse DATEI --resource cpu|memory|io [--host]",
    ),
    (
        "PSI metric is missing or duplicated",
        "PSI-Messgröße fehlt oder ist mehrfach vorhanden",
    ),
    ("invalid PSI metric", "ungültige PSI-Messgröße"),
    (
        "PSI input exceeds the size limit",
        "PSI-Eingabe überschreitet die Größenbegrenzung",
    ),
    (
        "remote workload PSI is unavailable locally",
        "PSI entfernter Jobs ist lokal nicht verfügbar",
    ),
    (
        "PSI requires an active local workload",
        "PSI erfordert einen aktiven lokalen Job",
    ),
    (
        "workload cgroup PSI is unavailable",
        "PSI der Job-cgroup ist nicht verfügbar",
    ),
    ("PSI scope", "PSI-Bereich"),
    ("host", "Host"),
    ("attempt", "Versuch"),
    (
        "sample time (Unix milliseconds)",
        "Messzeit (Unix-Millisekunden)",
    ),
    ("unavailable", "nicht verfügbar"),
    ("undefined for host CPU", "für Host-CPU nicht definiert"),
    ("no such Job", "kein solcher Job"),
    (
        "Pressure describes affected work, not the cause of contention. Observation changes no admission policy.",
        "Druck beschreibt betroffene Arbeit, nicht die Ursache der Konkurrenz. Die Messung ändert keine Zulassungsrichtlinie.",
    ),
];

pub fn message(key: &str) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    if locale.starts_with("de") {
        CATALOGUE
            .iter()
            .find(|(en, _)| *en == key)
            .map_or(key, |(_, de)| *de)
            .to_owned()
    } else {
        key.to_owned()
    }
}

pub fn help() -> String {
    ["usage: job pressure [ID] [--json]", "usage: job pressure status|events [--json]", "usage: job pressure replay FILE [--json]", "usage: job pressure parse FILE --resource cpu|memory|io [--host]", "usage: job queue|group create|set PATH --pressure FILE; unset PATH pressure", "Pressure describes affected work, not the cause of contention. Observation changes no admission policy."].map(message).join("\n")
}

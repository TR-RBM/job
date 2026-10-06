const CATALOGUE: &[(&str, &str)] = &[
    (
        "`{word}` is not a Job ID; write a positive number, a range such as 1-10 or a list such as 1,4,7-9",
        "`{word}` ist keine Job-ID; schreib eine positive Zahl, einen Bereich wie 1-10 oder eine Liste wie 1,4,7-9",
    ),
    (
        "`{word}` has an empty item; write IDs and ranges with one comma between them",
        "`{word}` enthält einen leeren Eintrag; schreib IDs und Bereiche mit je einem Komma dazwischen",
    ),
    (
        "`{word}` is an open range; write both ends, as in 5-10",
        "`{word}` ist ein offener Bereich; schreib beide Enden, wie in 5-10",
    ),
    (
        "`{word}` is a reversed range; write the lower ID first",
        "`{word}` ist ein umgekehrter Bereich; schreib die kleinere ID zuerst",
    ),
    (
        "the set names more than {limit} IDs; split it into several calls",
        "die Menge nennt mehr als {limit} IDs; teile sie auf mehrere Aufrufe auf",
    ),
    (
        "a set of Job IDs is needed",
        "eine Menge von Job-IDs wird gebraucht",
    ),
    (
        "the set `{word}` selects no existing Job",
        "die Menge `{word}` wählt keinen vorhandenen Job aus",
    ),
    (
        "dry run, nothing changed",
        "Probelauf, nichts wurde geändert",
    ),
    (
        "job {command} takes one or more sets of Job IDs; see job {command} --help",
        "job {command} nimmt eine oder mehrere Mengen von Job-IDs; siehe job {command} --help",
    ),
    (
        "job {command} needs --queue PATH; see job {command} --help",
        "job {command} braucht --queue PFAD; siehe job {command} --help",
    ),
    (
        "job {command} needs --priority N; see job {command} --help",
        "job {command} braucht --priority N; siehe job {command} --help",
    ),
    (
        "--summary shows the output of one Job; leave it out when waiting for a set",
        "--summary zeigt die Ausgabe eines einzelnen Jobs; lass es weg, wenn du auf eine Menge wartest",
    ),
    (
        "--format tsv and --id cannot be combined with an operand",
        "--format tsv und --id lassen sich nicht mit einem Operanden verbinden",
    ),
    ("exit status", "Rückgabewert"),
    ("cancelled", "abgebrochen"),
    ("stopping", "wird angehalten"),
    ("already ended", "schon beendet"),
    ("no such Job", "kein solcher Job"),
    ("removed", "entfernt"),
    ("released", "freigegeben"),
    ("retried", "neu versucht"),
    ("suspended", "ausgesetzt"),
    ("continued", "fortgesetzt"),
    ("signalled", "Signal gesendet"),
    ("reprioritized", "Priorität geändert"),
    ("moved", "verschoben"),
    ("refused", "abgelehnt"),
    ("failed", "fehlgeschlagen"),
    ("pending", "ausstehend"),
    ("still running", "läuft noch"),
    ("succeeded", "erfolgreich"),
    ("lost", "verloren"),
    ("held", "zurückgehalten"),
    ("queued", "eingereiht"),
    ("starting", "startet"),
    ("running", "läuft"),
];

pub fn message(key: &str) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
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

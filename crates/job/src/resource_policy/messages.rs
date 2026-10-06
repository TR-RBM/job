const CATALOGUE: &[(&str, &str)] = &[
    (
        "unknown recorded kernel resource control",
        "unbekannte gespeicherte Kernel-Ressourcenregel",
    ),
    ("I/O device", "I/O-Gerät"),
    ("available", "verfügbar"),
    (
        "I/O bandwidth requires an exact whole-byte rate or unlimited",
        "die I/O-Bandbreite erfordert eine exakte ganze Byte-Rate oder unlimited",
    ),
    (
        "I/O rates require positive finite values or unlimited",
        "I/O-Raten erfordern positive endliche Werte oder unlimited",
    ),
    (
        "I/O device requires a canonical MAJOR:MINOR number",
        "das I/O-Gerät erfordert eine kanonische MAJOR:MINOR-Nummer",
    ),
    (
        "an I/O policy needs at least one device; use unset to remove it",
        "eine I/O-Regel braucht mindestens ein Gerät; zum Entfernen verwende unset",
    ),
    (
        "I/O weight is outside the backend range",
        "das I/O-Gewicht liegt außerhalb des Backend-Bereichs",
    ),
    (
        "io-max requires DEVICE,rbps=RATE,wbps=RATE,riops=N,wiops=N",
        "io-max erfordert GERÄT,rbps=RATE,wbps=RATE,riops=N,wiops=N",
    ),
    (
        "unknown or repeated I/O rate field",
        "unbekanntes oder mehrfach angegebenes I/O-Ratenfeld",
    ),
    (
        "IOPS requires a positive integer or unlimited",
        "IOPS erfordert eine positive ganze Zahl oder unlimited",
    ),
    (
        "io-max needs at least one rate",
        "io-max braucht mindestens eine Rate",
    ),
    (
        "io-weight requires DEVICE=WEIGHT",
        "io-weight erfordert GERÄT=GEWICHT",
    ),
    (
        "each device may appear only once per I/O option",
        "jedes Gerät darf pro I/O-Option nur einmal vorkommen",
    ),
    (
        "I/O device is unavailable",
        "das I/O-Gerät ist nicht verfügbar",
    ),
    (
        "I/O device identity differs from the requested device",
        "die I/O-Geräteidentität weicht vom angeforderten Gerät ab",
    ),
    (
        "I/O maximum controller is unavailable",
        "der Controller für I/O-Obergrenzen ist nicht verfügbar",
    ),
    (
        "IOCost is not enabled for this device",
        "IOCost ist für dieses Gerät nicht aktiviert",
    ),
    (
        "BFQ requires the active scheduler and low_latency=0",
        "BFQ muss der aktive Scheduler sein und low_latency=0 verwenden",
    ),
    (
        "I/O controls require a whole block device, not a partition",
        "I/O-Regeln erfordern ein vollständiges Blockgerät und keine Partition",
    ),
    (
        "invalid recorded I/O policy",
        "ungültige gespeicherte I/O-Regel",
    ),
    (
        "I/O rates use bytes/s and operations/s on whole block devices. io-weight requires enabled IOCost; io-bfq-weight requires active BFQ with low_latency=0. Inspect io_devices in host --json. Device-wide policy is never changed automatically.",
        "I/O-Raten verwenden Bytes/s und Operationen/s auf vollständigen Blockgeräten. io-weight braucht aktiviertes IOCost; io-bfq-weight braucht aktives BFQ mit low_latency=0. Prüfe io_devices mit host --json. Geräteweite Regeln werden nie automatisch geändert.",
    ),
    (
        "{count} processes in cgroups that this Job made below its own were killed for memory; the command itself exited 0",
        "{count} Prozesse in cgroups, die dieser Job unterhalb seiner eigenen angelegt hat, wurden wegen Speichermangel beendet; der Befehl selbst endete mit 0",
    ),
    (
        "stopped: kernel OOM kill; inspect Job, aggregate and enclosing memory controls",
        "beendet: Kernel-OOM-Kill; prüfe die Speicherregeln des Jobs, der gemeinsamen Bereiche und der übergeordneten cgroups",
    ),
    (
        "Collection controls without job- apply to the entire subtree. Empty objects create no kernel domains. Inspect all ancestor constraints with queue/group show --json. Changes to active domains require a separate live-update operation.",
        "Sammlungsregeln ohne job- gelten für den gesamten Teilbaum. Leere Objekte erzeugen keine Kernel-Bereiche. Mit queue/group show --json siehst du alle übergeordneten Grenzen. Änderungen aktiver Bereiche erfordern eine gesonderte Live-Aktualisierung.",
    ),
    (
        "use unset to remove an aggregate control",
        "verwende unset, um eine gemeinsame Ressourcengrenze zu entfernen",
    ),
    (
        "aggregate controls require local execution",
        "gemeinsame Ressourcengrenzen erfordern lokale Ausführung",
    ),
    (
        "aggregate resource domain is unavailable",
        "gemeinsamer Ressourcenbereich ist nicht verfügbar",
    ),
    (
        "aggregate resource control changed outside the service",
        "gemeinsame Ressourcensteuerung wurde außerhalb des Dienstes geändert",
    ),
    (
        "aggregate resource domain is still populated",
        "im gemeinsamen Ressourcenbereich laufen noch Prozesse",
    ),
    (
        "unknown child in aggregate resource domain",
        "unbekannter Unterbereich im gemeinsamen Ressourcenbereich",
    ),
    (
        "aggregate policy changes require an inactive subtree",
        "Änderungen gemeinsamer Ressourcenregeln erfordern einen inaktiven Teilbaum",
    ),
    (
        "recovery requires the original aggregate resource domains",
        "Wiederaufnahme erfordert die ursprünglichen gemeinsamen Ressourcenbereiche",
    ),
    (
        "unsupported local request protocol",
        "nicht unterstütztes lokales Anfrageprotokoll",
    ),
    (
        "use unset to remove a resource default",
        "verwende unset, um eine Ressourcenvorgabe zu entfernen",
    ),
    (
        "Independent resource controls (all optional):",
        "Unabhängige Ressourcensteuerung (alles optional):",
    ),
    (
        "Requests affect admission only. Limits and weights require delegated cgroups. Collection per-Job defaults use --job- before these option names; unset restores inheritance.",
        "Requests beeinflussen nur die Zulassung. Grenzen und Gewichte brauchen delegierte cgroups. Für Job-Vorgaben einer Sammlung setzt du --job- vor diese Optionsnamen; unset stellt die Vererbung wieder her.",
    ),
    (
        "memory values require an exact whole-byte size within range",
        "Speicherwerte brauchen eine exakt ganze Byte-Anzahl im gültigen Bereich",
    ),
    (
        "recovery requires the original resource-control backend",
        "die Wiederaufnahme braucht das ursprüngliche Backend zur Ressourcensteuerung",
    ),
    (
        "kernel resource value differs from the requested value",
        "der Ressourcenwert im Kernel weicht vom angeforderten Wert ab",
    ),
    (
        "CPU values require a nonnegative decimal with at most three fractional digits",
        "CPU-Werte brauchen eine nichtnegative Dezimalzahl mit höchstens drei Nachkommastellen",
    ),
    (
        "a limit requires a nonnegative value or unlimited",
        "eine Grenze braucht einen nichtnegativen Wert oder unlimited",
    ),
    (
        "cpu-weight must be between 1 and 10000",
        "cpu-weight muss zwischen 1 und 10000 liegen",
    ),
    (
        "cpu-limit must be at least 0.01 CPU and fit the quota range",
        "cpu-limit muss mindestens 0,01 CPU betragen und in den Quotenbereich passen",
    ),
    (
        "memory limit cannot be rounded to a page safely",
        "die Speichergrenze lässt sich nicht sicher auf eine Seitengröße runden",
    ),
    (
        "legacy resource option conflicts with an explicit value",
        "die bisherige Ressourcenoption widerspricht einem expliziten Wert",
    ),
    (
        "requested resource control is unavailable",
        "die angeforderte Ressourcensteuerung ist nicht verfügbar",
    ),
    (
        "pids-max requires a positive whole number or unlimited",
        "pids-max benötigt eine positive ganze Zahl oder unlimited",
    ),
];

pub fn message(english: &str) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    if locale.starts_with("de") {
        CATALOGUE
            .iter()
            .find(|(key, _)| *key == english)
            .map_or(english, |(_, value)| *value)
            .to_owned()
    } else {
        english.to_owned()
    }
}

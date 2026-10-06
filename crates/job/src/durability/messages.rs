const CATALOGUE: &[(&str, &str)] = &[
    (
        "lost: the host restarted while the Job {state}; that boot began at {then}, this one at {now}",
        "verloren: der Rechner wurde neu gestartet, während der Job {state}; der frühere Start des Rechners war um {then}, der jetzige um {now}",
    ),
    (
        "lost: the host restarted while the Job {state}; this boot began at {now}",
        "verloren: der Rechner wurde neu gestartet, während der Job {state}; der jetzige Start des Rechners war um {now}",
    ),
    (
        "lost: the host restarted while the Job {state}",
        "verloren: der Rechner wurde neu gestartet, während der Job {state}",
    ),
    ("was starting", "gerade startete"),
    ("was running", "lief"),
    ("was suspended", "angehalten war"),
    ("was stopping", "gerade beendet wurde"),
    (
        "unknown failpoint in JOB_FAILPOINT",
        "unbekannter Failpoint in JOB_FAILPOINT",
    ),
    (
        "injected failure at failpoint",
        "eingespielter Fehler am Failpoint",
    ),
    (
        "cannot record the confirmed launch; the command was not started",
        "der bestätigte Start lässt sich nicht festhalten; das Kommando wurde nicht gestartet",
    ),
    (
        "the launch could not be confirmed; the command never started",
        "der Start ließ sich nicht bestätigen; das Kommando ist nie gestartet",
    ),
    (
        "its supervisor ended before it confirmed the launch",
        "sein Supervisor endete, bevor er den Start bestätigt hat",
    ),
    (
        "cannot record the exit status",
        "der Exit-Status lässt sich nicht festhalten",
    ),
    (
        "its supervisor ended after the command exited and before cleanup completed; leftover processes, usage and the end of the output were not recorded",
        "sein Supervisor endete, nachdem das Kommando fertig war und bevor das Aufräumen abgeschlossen war; übrige Prozesse, Verbrauch und das Ende der Ausgabe wurden nicht festgehalten",
    ),
    (
        "cannot write the result",
        "das Ergebnis lässt sich nicht schreiben",
    ),
    (
        "cannot write the final record",
        "der abschließende Datensatz lässt sich nicht schreiben",
    ),
    (
        "audit keep_files and rotate_bytes must be at least 1",
        "audit keep_files und rotate_bytes müssen mindestens 1 sein",
    ),
    ("invalid audit journal", "ungültiges Audit-Journal"),
    (
        "cannot write the audit record",
        "der Audit-Eintrag lässt sich nicht schreiben",
    ),
    (
        "usage: job audit [--target ID|PATH] [--action NAME] [--since MS] [--json]",
        "Aufruf: job audit [--target ID|PATH] [--action NAME] [--since MS] [--json]",
    ),
    (
        "audit lines that could not be read, such as a line cut short by a crash",
        "Audit-Zeilen, die sich nicht lesen ließen, etwa eine durch einen Absturz abgeschnittene Zeile",
    ),
    (
        "Reliability: --idempotency-key KEY on run, submit and create returns the Job already recorded for KEY instead of adding another; KEY is 1 to 128 characters of A-Z a-z 0-9 . _ : -",
        "Zuverlässigkeit: --idempotency-key KEY bei run, submit und create liefert den Job, der für KEY schon festgehalten ist, statt einen weiteren anzulegen; KEY besteht aus 1 bis 128 Zeichen aus A-Z a-z 0-9 . _ : -",
    ),
    (
        "job audit [--target ID|PATH] [--action NAME] [--since MS] [--json]  read the audit journal of mutating requests",
        "job audit [--target ID|PATH] [--action NAME] [--since MS] [--json]  lies das Audit-Journal der ändernden Anfragen",
    ),
    (
        "--idempotency-key needs 1 to 128 characters of A-Z a-z 0-9 . _ : -",
        "--idempotency-key braucht 1 bis 128 Zeichen aus A-Z a-z 0-9 . _ : -",
    ),
    ("invalid idempotency index", "ungültiger Idempotenz-Index"),
    (
        "this idempotency key already belongs to a Job with a different specification",
        "dieser Idempotenz-Schlüssel gehört schon zu einem Job mit einer anderen Spezifikation",
    ),
    (
        "--idempotency-key goes with run, submit and create",
        "--idempotency-key gehört zu run, submit und create",
    ),
    (
        "this idempotency key was already recorded; no new Job was added",
        "dieser Idempotenz-Schlüssel war schon festgehalten; es wurde kein neuer Job angelegt",
    ),
    (
        "this job speaks request protocol {own_min} to {own_max} (version {own_version}); the daemon speaks {min} to {max} (version {version}); restart the daemon from the same release, or install it for both",
        "dieses job spricht das Anfrageprotokoll {own_min} bis {own_max} (Version {own_version}); der Daemon spricht {min} bis {max} (Version {version}); starte den Daemon aus demselben Release neu oder installiere es für beide",
    ),
    (
        "removed leftover temporary files of the idempotency index",
        "übrig gebliebene temporäre Dateien des Idempotenz-Index entfernt",
    ),
    (
        "cannot remove leftover temporary files of the idempotency index",
        "übrig gebliebene temporäre Dateien des Idempotenz-Index lassen sich nicht entfernen",
    ),
    (
        "the service cannot write its audit journal, so it changes nothing; free space or repair the audit directory in the state directory and try again",
        "der Dienst kann sein Audit-Journal nicht schreiben und ändert deshalb nichts; schaffe Platz oder repariere das Verzeichnis audit im Zustandsverzeichnis und versuche es noch einmal",
    ),
    (
        "this service does not know request {name}; it speaks request protocol {min} to {max} (version {version}) and the request came as protocol {protocol}",
        "dieser Dienst kennt die Anfrage {name} nicht; er spricht das Anfrageprotokoll {min} bis {max} (Version {version}), und die Anfrage kam als Protokoll {protocol}",
    ),
    (
        "this service cannot read request {name} ({error}); it speaks request protocol {min} to {max} (version {version}) and the request came as protocol {protocol}",
        "dieser Dienst kann die Anfrage {name} nicht lesen ({error}); er spricht das Anfrageprotokoll {min} bis {max} (Version {version}), und die Anfrage kam als Protokoll {protocol}",
    ),
    (
        "the launch could not be recorded; the Job stays queued",
        "der Start ließ sich nicht festhalten; der Job bleibt in der Warteschlange",
    ),
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

const CATALOGUE: &[(&str, &str)] = &[
    (
        "events keep_files and rotate_bytes must be at least 1",
        "events keep_files und rotate_bytes müssen mindestens 1 sein",
    ),
    ("invalid event journal", "ungültiges Ereignisjournal"),
    (
        "cannot write the event record",
        "der Ereigniseintrag kann nicht geschrieben werden",
    ),
    (
        "cannot read the event journal",
        "das Ereignisjournal kann nicht gelesen werden",
    ),
    (
        "usage: job events [--follow] [--job ID] [--queue PATH] [--since MS] [--format json|text]",
        "Aufruf: job events [--follow] [--job ID] [--queue PATH] [--since MS] [--format json|text]",
    ),
    (
        "{count} event records were removed by rotation before they were read",
        "{count} Ereigniseinträge wurden durch die Rotation entfernt, bevor sie gelesen wurden",
    ),
    (
        "event lines that could not be read, such as a line cut short by a crash",
        "Ereigniszeilen, die nicht lesbar waren, etwa eine durch einen Absturz abgeschnittene Zeile",
    ),
    (
        "up {uptime}, {active} active and {waiting} waiting Jobs, {adopted} supervisors adopted at the last start, {free} left in the state directory, both journals writable",
        "läuft seit {uptime}, {active} aktive und {waiting} wartende Jobs, {adopted} Supervisoren beim letzten Start übernommen, {free} frei im Zustandsverzeichnis, beide Journale schreibbar",
    ),
    (
        "the service answered without a health report; it is an older build",
        "der Dienst hat ohne Zustandsbericht geantwortet; er ist ein älterer Stand",
    ),
    (
        "no health report, because the service does not answer",
        "kein Zustandsbericht, weil der Dienst nicht antwortet",
    ),
    (
        "restart the service after installing, so that it reports its health",
        "starte den Dienst nach der Installation neu, damit er seinen Zustand meldet",
    ),
    (
        "free space in the state directory and check that the service user can write audit/ and events/ in it; then run job doctor again",
        "schaffe Platz im Zustandsverzeichnis und prüfe, dass der Dienstbenutzer darin audit/ und events/ schreiben kann; führe dann job doctor erneut aus",
    ),
    (
        "capability error: this service only monitors, it has no delegated cgroup, so {limits} cannot be enforced; delegate a cgroup to the service (job doctor says how) or leave the option out",
        "Fähigkeitsfehler: dieser Dienst überwacht nur, er hat keine delegierte cgroup, deshalb lässt sich {limits} nicht durchsetzen; delegiere dem Dienst eine cgroup (job doctor sagt dir, wie) oder lass die Option weg",
    ),
    (
        "enforcing mode: limits are enforced through the delegated cgroup ([cgroup] required = {required})",
        "Durchsetzungsmodus: Grenzen werden über die delegierte cgroup durchgesetzt ([cgroup] required = {required})",
    ),
    (
        "monitoring mode: no delegated cgroup, limits are watched, not enforced; [cgroup] required = true makes the service refuse to start instead",
        "Überwachungsmodus: keine delegierte cgroup, Grenzen werden beobachtet, nicht durchgesetzt; mit [cgroup] required = true verweigert der Dienst stattdessen den Start",
    ),
    (
        "[cgroup] required = true, and no usable delegated cgroup was found",
        "[cgroup] required = true, und es wurde keine nutzbare delegierte cgroup gefunden",
    ),
    (
        "the cgroup of the service is not writable by it, or it is the root of the hierarchy",
        "die cgroup des Dienstes ist für ihn nicht schreibbar, oder sie ist die Wurzel der Hierarchie",
    ),
    (
        "delegate a cgroup to the service (Delegate=yes in its systemd unit, or a directory the service user owns that holds only the service), name one with [cgroup] root, or set required = false to run in monitoring mode",
        "delegiere dem Dienst eine cgroup (Delegate=yes in seiner systemd-Unit oder ein Verzeichnis des Dienstbenutzers, in dem nur der Dienst läuft), nenne eine mit [cgroup] root, oder setze required = false für den Überwachungsmodus",
    ),
    (
        "monitoring mode: the service has no delegated cgroup; limits are watched, not enforced, and with the ordinary profile a Job that asks for --mem, --pids or a cgroup control is refused",
        "Überwachungsmodus: der Dienst hat keine delegierte cgroup; Grenzen werden beobachtet, nicht durchgesetzt, und mit dem Profil ordinary wird ein Job abgelehnt, der --mem, --pids oder eine cgroup-Steuerung verlangt",
    ),
    (
        "enforcing mode: the service enforces limits through its delegated cgroup",
        "Durchsetzungsmodus: der Dienst setzt Grenzen über seine delegierte cgroup durch",
    ),
    (
        "delegate a cgroup to the service, see jobd(8); set [cgroup] required = true so that the service refuses to start without one",
        "delegiere dem Dienst eine cgroup, siehe jobd(8); setze [cgroup] required = true, damit der Dienst ohne eine den Start verweigert",
    ),
    (
        "the mode is known only from a running service",
        "der Modus ist nur von einem laufenden Dienst bekannt",
    ),
    ("not discoverable", "nicht feststellbar"),
    ("unlimited", "unbegrenzt"),
    (
        "{name} {value} set by {path}",
        "{name} {value}, gesetzt von {path}",
    ),
    (
        "surrounding cgroup {cgroup}: {limits}",
        "umgebende cgroup {cgroup}: {limits}",
    ),
    (
        "surrounding rlimits of the service (soft/hard): {limits}",
        "umgebende rlimits des Dienstes (weich/hart): {limits}",
    ),
    (
        "limits of this command, not of a running service: {limits}",
        "Grenzen dieses Befehls, nicht eines laufenden Dienstes: {limits}",
    ),
    (
        "the cgroup of the service cannot be read, so limits set around it are not discoverable; {limits}",
        "die cgroup des Dienstes ist nicht lesbar, deshalb sind die Grenzen um ihn herum nicht feststellbar; {limits}",
    ),
    (
        "mount cgroup v2 at /sys/fs/cgroup readable for the service user",
        "hänge cgroup v2 unter /sys/fs/cgroup so ein, dass der Dienstbenutzer es lesen kann",
    ),
    (
        "the output of this Job is served by the service, which does not answer at {socket}: {error}",
        "die Ausgabe dieses Jobs liefert der Dienst, und der antwortet unter {socket} nicht: {error}",
    ),
    (
        "the service does not serve output; restart it from the same release as this command",
        "der Dienst liefert keine Ausgabe; starte ihn aus demselben Stand wie diesen Befehl neu",
    ),
    (
        "the service stopped serving the output: {error}",
        "der Dienst hat aufgehört, die Ausgabe zu liefern: {error}",
    ),
    ("none", "keine"),
    ("unknown", "unbekannt"),
    ("writable", "schreibbar"),
    ("not writable", "nicht schreibbar"),
    (
        "jobs: {held} held, {queued} queued, {starting} starting, {running} running, {suspended} suspended, {stopping} stopping",
        "Jobs: {held} gehalten, {queued} wartend, {starting} startend, {running} laufend, {suspended} angehalten, {stopping} endend",
    ),
    (
        "oldest queued Job waits since: {age}",
        "ältester wartender Job wartet seit: {age}",
    ),
    (
        "started in the last hour: {count} Jobs, median wait {median}, longest wait {max}",
        "in der letzten Stunde gestartet: {count} Jobs, mittlere Wartezeit (Median) {median}, längste Wartezeit {max}",
    ),
    (
        "the audit journal cannot be written",
        "das Audit-Journal kann nicht geschrieben werden",
    ),
    (
        "the event journal cannot be written",
        "das Ereignisjournal kann nicht geschrieben werden",
    ),
    (
        "{count} cancellations could not be completed and are tried again",
        "{count} Abbrüche ließen sich nicht abschließen und werden erneut versucht",
    ),
    (
        "health: the starter failed {count} times since the service started; cancellations failing {failing}, retried {retries} times",
        "health: der Starter ist seit dem Start des Dienstes {count}-mal fehlgeschlagen; fehlschlagende Abbrüche {failing}, {retries}-mal erneut versucht",
    ),
    (
        "the state directory has less than {floor} left",
        "im Zustandsverzeichnis sind weniger als {floor} frei",
    ),
    (
        "limits are enforced through the delegated cgroup",
        "Grenzen werden über die delegierte cgroup durchgesetzt",
    ),
    (
        "monitoring only: no delegated cgroup, limits are watched, not enforced",
        "nur Überwachung: keine delegierte cgroup, Grenzen werden beobachtet, nicht durchgesetzt",
    ),
    (
        "health: up {uptime}, state schema {schema}, protocol {protocol}",
        "Zustand: läuft seit {uptime}, Zustandsschema {schema}, Protokoll {protocol}",
    ),
    ("health: jobs {counts}", "Zustand: Jobs {counts}"),
    (
        "health: last start adopted {adopted} supervisors and changed {changed} records",
        "Zustand: der letzte Start übernahm {adopted} Supervisoren und änderte {changed} Einträge",
    ),
    (
        "health: state directory has {free} left; audit journal {audit}, event journal {events}",
        "Zustand: im Zustandsverzeichnis sind {free} frei; Audit-Journal {audit}, Ereignisjournal {events}",
    ),
];

pub fn message(english: &str, values: &[(&str, String)]) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    let template = if locale.starts_with("de") {
        CATALOGUE
            .iter()
            .find(|(key, _)| *key == english)
            .map_or(english, |(_, value)| *value)
    } else {
        english
    };
    values
        .iter()
        .fold(template.to_owned(), |text, (key, value)| {
            text.replace(&format!("{{{key}}}"), value)
        })
}

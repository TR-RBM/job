const HELP: [&str; 3] = [
    "Optional isolation: --namespaces user,mount,pid,ipc,uts,cgroup|inherit; --root read-only|inherit; --private-tmp yes|inherit; --writable PATH,...|inherit",
    "Nothing is implied: --root, --private-tmp and --writable need --namespaces user,mount, pid needs mount, and --writable needs --root read-only. Queue and Group defaults use the --job- prefix. Inspect the host with host --json under isolation_controls.",
    "A mount namespace is not a container and jobs of one user are not separated from each other. With pid the job sees only its own processes under a small init, and they all end with it; that hides processes and is not a security boundary.",
];
const CATALOGUE: &[(&str, &str)] = &[
    ("invalid namespace list", "ungültige Namespace-Liste"),
    (
        "pid needs mount in --namespaces, because the job gets its own /proc; add mount",
        "pid braucht mount in --namespaces, weil der Job ein eigenes /proc bekommt; ergänze mount",
    ),
    (
        "this host cannot start a job in a pid namespace: {error}",
        "dieser Host kann keinen Job in einem pid-Namespace starten: {error}",
    ),
    (
        "start the init of the pid namespace",
        "den init-Prozess des pid-Namespace starten",
    ),
    ("mount a fresh /proc", "ein neues /proc einhängen"),
    (
        "read back the pid namespace",
        "den pid-Namespace zurücklesen",
    ),
    (
        "start the command under the init",
        "das Kommando unter dem init-Prozess starten",
    ),
    (
        "the network namespace is requested with --net, not with --namespaces",
        "den Netzwerk-Namespace forderst du mit --net an, nicht mit --namespaces",
    ),
    (
        "--writable takes up to 32 absolute paths separated by commas, without . or .. and not / itself",
        "--writable nimmt bis zu 32 absolute Pfade, durch Kommas getrennt, ohne . oder .. und nicht / selbst",
    ),
    (
        "{option} needs both mount and user in --namespaces; add --namespaces user,mount",
        "{option} braucht mount und user in --namespaces; ergänze --namespaces user,mount",
    ),
    (
        "--writable needs --root read-only; without it every path is already writable as before",
        "--writable braucht --root read-only; ohne das ist jeder Pfad schon wie bisher beschreibbar",
    ),
    (
        "--private-tmp yes hides {path}; choose a writable path outside /tmp and /var/tmp",
        "--private-tmp yes verdeckt {path}; wähle einen beschreibbaren Pfad außerhalb von /tmp und /var/tmp",
    ),
    (
        "--root requires read-only or inherit",
        "--root benötigt read-only oder inherit",
    ),
    (
        "--private-tmp requires yes or inherit",
        "--private-tmp benötigt yes oder inherit",
    ),
    ("duplicate isolation control", "doppelte Isolationsoption"),
    (
        "--private-tmp yes hides the working directory {path}; choose a working directory outside /tmp and /var/tmp",
        "--private-tmp yes verdeckt das Arbeitsverzeichnis {path}; wähle ein Arbeitsverzeichnis außerhalb von /tmp und /var/tmp",
    ),
    (
        "this service runs as root, so a job keeps the capability to undo --root read-only and --private-tmp; add --cap-drop sys_admin or --cap-drop all",
        "dieser Dienst läuft als root, deshalb behält ein Job die Fähigkeit, --root read-only und --private-tmp aufzuheben; ergänze --cap-drop sys_admin oder --cap-drop all",
    ),
    (
        "the private tmp could not be removed: {error}; it is kept in {path}",
        "das private tmp ließ sich nicht entfernen: {error}; es liegt jetzt in {path}",
    ),
    (
        "it is nested deeper than 128 directories",
        "es ist tiefer als 128 Verzeichnisse verschachtelt",
    ),
    (
        "invalid applied isolation controls",
        "ungültige angewendete Isolation",
    ),
    (
        "requested isolation control is unavailable",
        "angeforderte Isolation ist nicht verfügbar",
    ),
    (
        "this host has no {kind} namespaces, so it cannot run a job with --namespaces {kind}",
        "dieser Host hat keine {kind}-Namespaces und kann deshalb keinen Job mit --namespaces {kind} ausführen",
    ),
    (
        "this host does not let a user create namespaces, so it cannot run a job with --namespaces user",
        "dieser Host lässt Benutzer keine Namespaces anlegen und kann deshalb keinen Job mit --namespaces user ausführen",
    ),
    (
        "this service is not privileged, so --namespaces needs user in its list; add user",
        "dieser Dienst hat keine Sonderrechte, deshalb braucht --namespaces user in der Liste; ergänze user",
    ),
    (
        "this host has no mount_setattr, so it cannot run a job with --root, --private-tmp or --writable",
        "dieser Host hat kein mount_setattr und kann deshalb keinen Job mit --root, --private-tmp oder --writable ausführen",
    ),
    (
        "the writable path {path} does not exist",
        "den beschreibbaren Pfad {path} gibt es nicht",
    ),
    (
        "this host has no /tmp to make private",
        "dieser Host hat kein /tmp, das privat werden könnte",
    ),
    (
        "isolation could not {step}: {error}",
        "die Isolation konnte nicht {step}: {error}",
    ),
    (
        "the kernel state does not match the request",
        "der Zustand im Kernel passt nicht zur Anforderung",
    ),
    ("create the user namespace", "den user-Namespace anlegen"),
    (
        "write the user namespace identity maps",
        "die Identitätszuordnung des user-Namespace schreiben",
    ),
    (
        "create the requested namespaces",
        "die angeforderten Namespaces anlegen",
    ),
    ("make mounts private", "die Mounts privat machen"),
    (
        "bind a writable path",
        "einen beschreibbaren Pfad einbinden",
    ),
    ("bind the private tmp", "das private tmp einbinden"),
    ("make the root read-only", "die Wurzel nur lesbar machen"),
    (
        "make a writable path or the private tmp writable again",
        "einen beschreibbaren Pfad oder das private tmp wieder beschreibbar machen",
    ),
    (
        "return to the working directory",
        "in das Arbeitsverzeichnis zurückkehren",
    ),
    ("read back the namespaces", "die Namespaces zurücklesen"),
    (
        "read back the read-only root",
        "die nur lesbare Wurzel zurücklesen",
    ),
    (
        "read back a writable path",
        "einen beschreibbaren Pfad zurücklesen",
    ),
    ("read back the private tmp", "das private tmp zurücklesen"),
    ("none", "keine"),
    ("yes", "ja"),
    ("no", "nein"),
    ("unavailable", "nicht verfügbar"),
    ("shared with the host", "mit dem Host geteilt"),
    ("per job", "je Job"),
    (
        "per job behind a link per Queue or per job",
        "je Job hinter einer Verbindung je Queue oder je Job",
    ),
    (
        "security controls: no_new_privs {nnp}, seccomp deny lists {seccomp} ({syscalls} system calls), {caps} capability names",
        "Sicherheitssteuerung: no_new_privs {nnp}, seccomp-Verbotslisten {seccomp} ({syscalls} Systemaufrufe), {caps} Capability-Namen",
    ),
    (
        "process controls: CPU affinity {affinity}, NUMA policy {numa}, {limits} resource limits",
        "Prozesssteuerung: CPU-Bindung {affinity}, NUMA-Richtlinie {numa}, {limits} Ressourcengrenzen",
    ),
    (
        "isolation: namespaces {kinds}; user namespaces without privilege {user}; read-only root and private tmp {mounts}; pid namespaces with an init and a fresh /proc {pid}",
        "Isolation: Namespaces {kinds}; user-Namespaces ohne Sonderrechte {user}; nur lesbare Wurzel und privates tmp {mounts}; pid-Namespaces mit init-Prozess und neuem /proc {pid}",
    ),
    (
        "isolation: Landlock ABI {landlock}; missing network tools: {tools}; IPv6 in a job's linked network {ipv6}",
        "Isolation: Landlock-ABI {landlock}; fehlende Netzwerkwerkzeuge: {tools}; IPv6 im verbundenen Netz eines Jobs {ipv6}",
    ),
    (
        "network namespaces: {modes}",
        "Netzwerk-Namespaces: {modes}",
    ),
    (
        HELP[0],
        "Optionale Isolation: --namespaces user,mount,pid,ipc,uts,cgroup|inherit; --root read-only|inherit; --private-tmp yes|inherit; --writable PATH,...|inherit",
    ),
    (
        HELP[1],
        "Nichts wird stillschweigend ergänzt: --root, --private-tmp und --writable brauchen --namespaces user,mount, pid braucht mount, und --writable braucht --root read-only. Standardwerte für Queues und Gruppen verwenden das Präfix --job-. Was der Host kann, zeigt dir host --json unter isolation_controls.",
    ),
    (
        HELP[2],
        "Ein mount-Namespace ist kein Container, und Jobs desselben Benutzers sind nicht voneinander getrennt. Mit pid sieht der Job nur seine eigenen Prozesse unter einem kleinen init-Prozess, und sie enden alle mit ihm; das verbirgt Prozesse und ist keine Sicherheitsgrenze.",
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
pub fn text(key: &str, values: &[(&str, String)]) -> String {
    values.iter().fold(message(key), |text, (name, value)| {
        text.replace(&format!("{{{name}}}"), value)
    })
}
pub fn help() -> String {
    HELP.map(message).join("\n")
}

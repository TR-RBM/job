# Security policy

## What counts as a vulnerability

job has a stated security model in [docs/security.md](docs/security.md). Please read it first. It says plainly that a Job runs as the Unix user of the service and that two Jobs of one service are not protected from each other. Behaviour that the model describes as not protected is a known limit, not a vulnerability.

A vulnerability is a case where job does less than that model and its documentation say. Examples:

- a client that is not admitted to the socket can make the service act or answer;
- a requested control (`--no-new-privs`, `--cap-drop`, `--seccomp-deny`, `--confine`, `--namespaces`, `--root read-only`, `--private-tmp`, `--net none`, a proxy or tunnel boundary) is reported as applied and is not, or a Job runs although the control could not be established;
- proxy credentials or other secrets appear in status output, logs, process arguments, the audit journal or error messages where the documentation says they do not;
- a request lets a client read or write files outside what the service user could reach anyway, or lets a Job gain privileges the service user does not have;
- crafted input to the command line, the socket, a state directory, a backup or a configuration file makes the service or a maintenance command write outside its own directories, or run something it was not asked to run;
- the service itself, when run with privilege, hands that privilege to a Job.

If you are unsure, report it.

## How to report

Report privately. Do not open a public issue, and do not publish details, before a fix is available.

Send the report by e-mail to the maintainer, Tim Richter, at `info@richter-it-service.eu`. Include:

- the version (`job host --json` has it under `version`) and how job was installed;
- the distribution, kernel version, service manager, and whether the service runs for one user or system-wide with a socket group;
- what you did, what you expected and what happened, with the exact commands where possible;
- the output of `job doctor` and `job host --json`, with anything private removed;
- whether the problem is already known to others.

Do not include real credentials or private data.

## What happens next

The maintainers confirm that the report arrived, examine it, and tell you whether they regard it as a vulnerability and why. A confirmed vulnerability is fixed in a new release, and the [release notes](docs/release-notes.md) of that release describe it and, if you wish, name you. Please allow time for a fix before you publish.

You get an answer within 72 hours of your report. That answer confirms that the report arrived and says what happens next; it is not a promise that a fix exists by then.

## Supported versions

Fixes are made on the current release. Earlier releases receive no fixes.

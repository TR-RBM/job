# Proxy credentials

## What it does

Keeps a proxy's user and password out of everything the service shows or records. `--net-secret-file FILE` names a file holding `USER:PASSWORD`; it must be a regular file of the service user with no group or other permission bits, and only its path is recorded. A proxy address that carries the user and password inline is still accepted: at submission they move into a private file, mode 600, in the Job's state directory, or under `net-secrets/` for a Queue or Group, and the record keeps the address as `scheme://***@HOST:PORT` beside that file's path. The supervisor reads the file at launch and adds the credentials to the Job's proxy variables only. A remote Job receives them inside the request sent over SSH, never as an argument. Addresses already stored inline in the object graph move the same way when the service starts. The same redaction is applied to the hook's denial log and to messages that quote an address.

## How to test

Run the `network_boundary_` service integration cases through job: `cargo test --test watch_backend network_boundary_`. The credential cases are `network_boundary_proxy_credentials_stay_out_of_answers_records_and_arguments`, `network_boundary_a_secret_file_is_checked_and_only_its_path_is_recorded`, `network_boundary_a_remote_job_carries_its_proxy_password_in_the_request_not_in_arguments`, `network_boundary_a_stored_queue_password_moves_to_a_private_file_at_start` and `network_boundary_the_hook_logs_a_denied_command_without_proxy_credentials`. No new unit tests.

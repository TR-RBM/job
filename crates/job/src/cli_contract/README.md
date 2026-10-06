# CLI execution and waiting contracts

## What it does

Defines the explicit compatibility switch, literal command arguments, shell selection and quiet wait outcomes. Shell choice is a client transformation into a literal executable/argument vector saved in the existing specification. Native wait keeps output empty unless JSON or a summary is selected. Versioned JSON distinguishes command exit codes from service errors, start failures, cancellation, lost execution and timeout.

## How to test

Run the unix_cli and unix_wait CLI/service integration cases through job, then the repository gate. Existing integration fixtures explicitly select the legacy interface where they test the earlier command and summary contract. The existing argv unit test follows the literal contract; no new unit tests are added. Separate live output streams are tracked as subsequent work in the same CLI redesign block.

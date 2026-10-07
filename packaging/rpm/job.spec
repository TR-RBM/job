%global debug_package %{nil}
%global __os_install_post %{nil}

Name:           job
Version:        %{job_version}
Release:        1
Summary:        Execution and scheduling service for Linux
License:        Apache-2.0
URL:            https://github.com/TR-RBM/job

%description
job runs commands as Jobs under a service that keeps them independent of the
terminal they came from and records what they printed and how they ended. Jobs
are organized into queues and groups, with reserved and limited resources,
namespaces, network boundaries and remote hosts over SSH.

%install
cp -a %{job_stage}/. %{buildroot}/

%files
/usr/bin/job
/usr/bin/jobd
/usr/share/man/man1/job.1*
/usr/share/man/man5/job.conf.5*
/usr/share/man/man7/job.7*
/usr/share/man/man8/jobd.8*
/usr/share/bash-completion/completions/job
/usr/share/fish/vendor_completions.d/job.fish
/usr/lib/systemd/system/jobd.service
/usr/lib/systemd/user/jobd.service
/usr/share/doc/job

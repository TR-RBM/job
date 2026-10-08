PREFIX ?= /usr/local
DESTDIR ?=
CARGO ?= cargo
INSTALL ?= install
JOB_BIN ?= target/release/job

bindir = $(PREFIX)/bin
mandir = $(PREFIX)/share/man
bashdir = $(PREFIX)/share/bash-completion/completions
fishdir = $(PREFIX)/share/fish/vendor_completions.d
zshdir = $(PREFIX)/share/zsh/site-functions
unitdir = $(PREFIX)/lib/systemd
docdir = $(PREFIX)/share/doc/job

FILES = \
	$(bindir)/job \
	$(bindir)/jobd \
	$(mandir)/man1/job.1 \
	$(mandir)/man5/job.conf.5 \
	$(mandir)/man7/job.7 \
	$(mandir)/man8/jobd.8 \
	$(bashdir)/job \
	$(fishdir)/job.fish \
	$(zshdir)/_job \
	$(unitdir)/system/jobd.service \
	$(unitdir)/user/jobd.service \
	$(docdir)/LICENSE \
	$(docdir)/administration.md \
	$(docdir)/migration.md \
	$(docdir)/runit/run

.PHONY: build install uninstall check

build:
	$(CARGO) build --release

install:
	$(INSTALL) -d $(DESTDIR)$(bindir) $(DESTDIR)$(mandir)/man1 $(DESTDIR)$(mandir)/man5 \
		$(DESTDIR)$(mandir)/man7 $(DESTDIR)$(mandir)/man8 $(DESTDIR)$(bashdir) \
		$(DESTDIR)$(fishdir) $(DESTDIR)$(zshdir) $(DESTDIR)$(unitdir)/system $(DESTDIR)$(unitdir)/user \
		$(DESTDIR)$(docdir)/runit
	$(INSTALL) -m 0755 $(JOB_BIN) $(DESTDIR)$(bindir)/job
	ln -sf job $(DESTDIR)$(bindir)/jobd
	$(INSTALL) -m 0644 man/job.1 $(DESTDIR)$(mandir)/man1/job.1
	$(INSTALL) -m 0644 man/job.conf.5 $(DESTDIR)$(mandir)/man5/job.conf.5
	$(INSTALL) -m 0644 man/job.7 $(DESTDIR)$(mandir)/man7/job.7
	$(INSTALL) -m 0644 man/jobd.8 $(DESTDIR)$(mandir)/man8/jobd.8
	$(INSTALL) -m 0644 completions/job.bash $(DESTDIR)$(bashdir)/job
	$(INSTALL) -m 0644 completions/job.fish $(DESTDIR)$(fishdir)/job.fish
	$(INSTALL) -m 0644 completions/job.zsh $(DESTDIR)$(zshdir)/_job
	sed 's|@bindir@|$(bindir)|g' deploy/systemd/jobd.service.in > $(DESTDIR)$(unitdir)/system/jobd.service
	chmod 0644 $(DESTDIR)$(unitdir)/system/jobd.service
	sed 's|@bindir@|$(bindir)|g' deploy/systemd/jobd-user.service.in > $(DESTDIR)$(unitdir)/user/jobd.service
	chmod 0644 $(DESTDIR)$(unitdir)/user/jobd.service
	sed 's|@bindir@|$(bindir)|g' deploy/runit/job/run.in > $(DESTDIR)$(docdir)/runit/run
	chmod 0755 $(DESTDIR)$(docdir)/runit/run
	$(INSTALL) -m 0644 LICENSE $(DESTDIR)$(docdir)/LICENSE
	$(INSTALL) -m 0644 docs/administration.md $(DESTDIR)$(docdir)/administration.md
	$(INSTALL) -m 0644 docs/migration.md $(DESTDIR)$(docdir)/migration.md

uninstall:
	rm -f $(addprefix $(DESTDIR),$(FILES))
	-rmdir $(DESTDIR)$(docdir)/runit $(DESTDIR)$(docdir)

check:
	tools/check

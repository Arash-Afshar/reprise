# Reprise install for Omarchy (Super+Space launcher via .desktop)
PREFIX          ?= $(HOME)/.local
BINDIR          ?= $(PREFIX)/bin
APPLICATIONSDIR ?= $(HOME)/.local/share/applications
BINARY          := target/release/reprise
DESKTOP         := $(APPLICATIONSDIR)/reprise.desktop

.PHONY: build install uninstall

build:
	cargo build --release

install: build
	mkdir -p "$(BINDIR)" "$(APPLICATIONSDIR)"
	install -m 755 "$(BINARY)" "$(BINDIR)/reprise"
	printf '%s\n' \
		'[Desktop Entry]' \
		'Version=1.0' \
		'Name=Reprise' \
		'Comment=Chess Reprise - local game postmortem' \
		'Exec=$(BINDIR)/reprise' \
		'Terminal=false' \
		'Type=Application' \
		'Categories=Game;BoardGame;' \
		'StartupNotify=true' \
		> "$(DESKTOP)"
	update-desktop-database "$(APPLICATIONSDIR)" 2>/dev/null || true
	@echo "Installed $(BINDIR)/reprise"
	@echo "Launcher: $(DESKTOP)"

uninstall:
	rm -f "$(BINDIR)/reprise" "$(DESKTOP)"
	update-desktop-database "$(APPLICATIONSDIR)" 2>/dev/null || true

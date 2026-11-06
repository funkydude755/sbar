PREFIX ?= /usr/local

build:
	cargo build --release --locked

test:
	cargo test --locked
	cargo build --locked
	python3 tests/test_pty.py target/debug/sbar

install: build
	install -d "$(PREFIX)/bin" "$(PREFIX)/share/licenses/sbar"
	install -m 755 target/release/sbar "$(PREFIX)/bin/sbar"
	install -m 644 LICENSE THIRD_PARTY_LICENSES.md "$(PREFIX)/share/licenses/sbar/"

uninstall:
	rm -f "$(PREFIX)/bin/sbar" "$(PREFIX)/share/licenses/sbar/LICENSE" "$(PREFIX)/share/licenses/sbar/THIRD_PARTY_LICENSES.md"

clean:
	cargo clean

.PHONY: build test install uninstall clean

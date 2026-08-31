BINARY := k9x
PREFIX ?= /usr/local
TARGET ?= release

.PHONY: build install uninstall universal clean bench

build:
	cargo build --release

install: build
	install -d $(PREFIX)/bin
	install -m 0755 target/release/$(BINARY) $(PREFIX)/bin/$(BINARY)
	@echo "installed → $(PREFIX)/bin/$(BINARY)"

uninstall:
	rm -f $(PREFIX)/bin/$(BINARY)

universal:
	rustup target add aarch64-apple-darwin x86_64-apple-darwin
	cargo build --release --target aarch64-apple-darwin
	cargo build --release --target x86_64-apple-darwin
	lipo -create -output target/$(BINARY)-macos-universal \
		target/aarch64-apple-darwin/release/$(BINARY) \
		target/x86_64-apple-darwin/release/$(BINARY)
	@echo "universal binary → target/$(BINARY)-macos-universal"

bench:
	@python3 scripts/bench.py || python3 bench.py 2>/dev/null || echo "(see README benchmark table)"

clean:
	cargo clean

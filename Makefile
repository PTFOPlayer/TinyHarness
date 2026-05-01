PREFIX ?= $(HOME)/.local/bin
BINARY  := tinyharness

.PHONY: build install uninstall clean

build:
	cargo build --release

install: build
	@echo "Installing $(BINARY) to $(PREFIX)..."
	install -m 755 target/release/$(BINARY) $(PREFIX)/$(BINARY)
	@echo "Done! You can now run 'tinyharness' from anywhere."

uninstall:
	@echo "Removing $(BINARY) from $(PREFIX)..."
	rm -f $(PREFIX)/$(BINARY)
	@echo "Done."

clean:
	cargo clean
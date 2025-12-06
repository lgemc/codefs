BINARY_NAME := codefs
BUILD_DIR := target/release
INSTALL_DIR := $(HOME)/.local/bin

.PHONY: build install clean

build:
	cargo build --release

install: build
	@mkdir -p $(INSTALL_DIR)
	cp $(BUILD_DIR)/$(BINARY_NAME) $(INSTALL_DIR)/$(BINARY_NAME)

clean:
	cargo clean

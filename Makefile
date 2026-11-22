.PHONY: check fmt fmt-check clippy test doc examples terminal-check deps package-check msrv audit

check: fmt-check clippy test doc deps package-check examples terminal-check

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

clippy:
	cargo clippy --locked --workspace --all-targets -- -D warnings

test:
	cargo test --locked --workspace

doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps

examples:
	cargo run --locked -q -p roomkit-tui-examples --bin assistant-demo -- --check
	cargo run --locked -q -p roomkit-tui-examples --bin order-support -- --check
	cargo run --locked -q -p roomkit-tui-examples --bin roomkit-chat -- --check

terminal-check:
	cargo build --locked -p roomkit-tui-examples --bins
	python3 scripts/terminal_screen.py
	python3 scripts/check-terminal.py
	python3 scripts/check-history.py
	python3 scripts/check-links.py
	python3 scripts/check-terminal-status.py
	python3 scripts/check-sidebar.py
	python3 scripts/check-selection.py
	python3 scripts/check-steering.py
	python3 scripts/check-thinking.py

deps:
	python3 scripts/check-boundaries.py

package-check:
	python3 scripts/check-packages.py

# The examples take a model provider that asks for Rust 1.94: the libraries'
# minimum is checked without them.
msrv:
	cargo +1.92.0 test --locked --workspace --exclude roomkit-tui-examples

audit:
	cargo deny $(DENY_FLAGS) check

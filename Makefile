.PHONY: build test lint format format-check hooks secrets install clean

build:
	python3 tools/build_zipapp.py

test:
	tests/run_tests.sh

lint:
	ruff check .

format:
	ruff format .

format-check:
	ruff format --check .

hooks:
	pre-commit install

secrets:
	pre-commit run gitleaks-history --hook-stage manual --all-files

install: build
	./install.sh

clean:
	rm -rf dist/qdu src/qdu/__pycache__ tests/__pycache__

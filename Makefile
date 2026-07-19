.PHONY: build test install clean

build:
	python3 tools/build_zipapp.py

test:
	tests/run_tests.sh

install: build
	./install.sh

clean:
	rm -rf dist/qdu src/qdu/__pycache__ tests/__pycache__

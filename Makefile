GOBIN := $(or $(shell go env GOBIN),$(shell go env GOPATH)/bin)

# Pinned here and in .github/workflows/ci.yml. Run with go run, not go tool,
# so their dependencies stay out of go.mod.
GOLANGCI_LINT := go run github.com/golangci/golangci-lint/v2/cmd/golangci-lint@v2.14.0
GOVULNCHECK := go run golang.org/x/vuln/cmd/govulncheck@v1.8.0
GORELEASER := go run github.com/goreleaser/goreleaser/v2@v2.18.2

.PHONY: build install uninstall test lint fmt vuln check hooks snapshot clean

build:
	go build -o bin/rvw ./cmd/rvw

install:
	go install ./cmd/rvw

uninstall:
	rm -f $(GOBIN)/rvw

test:
	go vet ./...
	go test ./...
	bats test/rvw.bats

lint:
	$(GOLANGCI_LINT) run ./...

fmt:
	$(GOLANGCI_LINT) fmt ./...

vuln:
	$(GOVULNCHECK) ./...

check: lint vuln test

hooks:
	git config core.hooksPath .githooks

# The release build without publishing, into dist/. The real one runs in CI
# when a v* tag is pushed.
snapshot:
	$(GORELEASER) check
	$(GORELEASER) release --snapshot --clean

clean:
	rm -rf bin dist

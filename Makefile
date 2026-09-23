GOBIN := $(or $(shell go env GOBIN),$(shell go env GOPATH)/bin)

.PHONY: build install uninstall test clean

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

clean:
	rm -rf bin

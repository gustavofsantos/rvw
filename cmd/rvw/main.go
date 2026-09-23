// Command rvw is a per-workspace queue of code review feedback between humans
// and agents. See `rvw --help`.
package main

import (
	"context"
	"os"
	"os/signal"
	"syscall"

	"github.com/gustavofsantos/rvw/internal/cli"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	code := cli.Main(ctx, os.Args[1:], os.Stdin, os.Stdout, os.Stderr)
	stop()
	os.Exit(code)
}

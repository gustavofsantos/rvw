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
	// The first signal cancels ctx; restoring the default handlers then lets a
	// second one end the process even if something ignores ctx.
	go func() {
		<-ctx.Done()
		stop()
	}()
	code := cli.Main(ctx, os.Args[1:], os.Stdin, os.Stdout, os.Stderr)
	stop()
	os.Exit(code)
}

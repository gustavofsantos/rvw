package cli

import (
	"errors"
	"fmt"
	"net"
	"net/http"
	"net/netip"
	"os"
	"os/exec"
	"path/filepath"
	"runtime/debug"
	"strings"
	"time"

	"github.com/modelcontextprotocol/go-sdk/mcp"
	"github.com/spf13/cobra"

	"github.com/gustavofsantos/rvw/internal/mcpserver"
	"github.com/gustavofsantos/rvw/internal/render"
)

const mcpPath = "/mcp"

// serverFlags are the defaults a server applies to calls that leave them empty.
type serverFlags struct {
	lane, author, http string
}

func (s *serverFlags) register(cmd *cobra.Command) {
	cmd.Flags().StringVar(&s.lane, "lane", "", "pin calls that name no lane to this lane (default: every lane)")
	cmd.Flags().StringVar(&s.author, "author", "", "author of calls that name none (default: $USER)")
	cmd.Flags().StringVar(&s.http, "http", "", "serve streamable HTTP on this loopback address (e.g. 127.0.0.1:7777) instead of stdio")
}

func (a *app) mcpCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "mcp",
		Short: "serve the queue as MCP tools, and print how to connect Claude Code",
		Long: `Serve the queue over the Model Context Protocol, for agents that prefer tool
calls to shell commands. Every tool is one rvw operation: add, submit, list,
list_reviews, count, pull, show_comment, show_review, resolve, edit and
workspaces. Each call names its workspace, so one server serves every project.

` + "`rvw mcp config`" + ` prints how to register the server with Claude Code.`,
		Args: cobra.NoArgs,
		Run:  func(cmd *cobra.Command, _ []string) { cmd.Help() },
	}
	cmd.AddCommand(a.mcpServeCmd(), a.mcpConfigCmd())
	return cmd
}

func (a *app) mcpServeCmd() *cobra.Command {
	var flags serverFlags
	cmd := &cobra.Command{
		Use:   "serve [--http ADDR]",
		Short: "run the MCP server on stdio, or on a local HTTP address",
		Long: `Run the MCP server. By default it speaks over stdin and stdout, the way
Claude Code starts a local server. With --http it listens on that address and
serves streamable HTTP at ` + mcpPath + `, for clients that connect to a running
server. The server has no authentication, so the address must be loopback
(127.0.0.1, ::1 or localhost).

A call that leaves workspace empty uses --workspace, else the git worktree the
server started in. Outside git the server refuses to start. --lane and
--author fill calls that leave them empty.`,
		Example: `  rvw mcp serve
  rvw mcp serve --author claude
  rvw mcp serve --http 127.0.0.1:7777`,
		Args: cobra.NoArgs,
		RunE: func(*cobra.Command, []string) error {
			if flags.http != "" {
				if err := checkLoopback(flags.http); err != nil {
					return err
				}
			}
			ws, err := a.workspace()
			if err != nil {
				return err
			}
			svc, err := a.svc()
			if err != nil {
				return err
			}
			server := mcpserver.New(svc, mcpserver.Options{
				Workspace: ws, Lane: laneOf(flags.lane), Author: authorOf(flags.author), Version: version(),
			})
			if flags.http == "" {
				return server.Run(a.ctx, &mcp.StdioTransport{})
			}
			return a.serveHTTP(server, flags.http)
		},
	}
	flags.register(cmd)
	return cmd
}

func (a *app) serveHTTP(server *mcp.Server, addr string) error {
	ln, err := net.Listen("tcp", addr)
	if err != nil {
		return err
	}
	mux := http.NewServeMux()
	mux.Handle(mcpPath, mcp.NewStreamableHTTPHandler(func(*http.Request) *mcp.Server { return server }, nil))
	srv := &http.Server{Handler: mux, ReadHeaderTimeout: 10 * time.Second}
	go func() {
		<-a.ctx.Done()
		srv.Close()
	}()
	a.notice("serving MCP at %s", mcpURL(ln.Addr().String()))
	if err := srv.Serve(ln); !errors.Is(err, http.ErrServerClosed) {
		return err
	}
	return nil
}

func (a *app) mcpConfigCmd() *cobra.Command {
	var (
		flags  serverFlags
		scope  string
		name   string
		format formatFlag
	)
	cmd := &cobra.Command{
		Use:   "config",
		Short: "print how to add the MCP server to Claude Code",
		Long: `Print the ` + "`claude mcp add`" + ` command, and the equivalent .mcp.json entry, that
register rvw as an MCP server in Claude Code. Nothing is changed; run the
command or paste the entry yourself.

By default Claude Code starts the server on stdio. With --http it connects to
a server you keep running with ` + "`rvw mcp serve --http ADDR`" + `. --db, --workspace,
--lane and --author are carried into the server command.`,
		Example: `  rvw mcp config
  rvw mcp config --author claude --scope project
  rvw mcp config --http 127.0.0.1:7777
  rvw mcp config --format json          # only the .mcp.json entry`,
		Args: cobra.NoArgs,
		RunE: func(*cobra.Command, []string) error {
			if err := format.check(); err != nil {
				return err
			}
			if err := oneOf("--scope", scope, []string{"local", "project", "user"}); err != nil {
				return err
			}
			if a.workspaceFlag != "" {
				if _, err := a.workspace(); err != nil {
					return err
				}
			}
			entry, add, err := a.mcpEntry(flags, scope, name)
			if err != nil {
				return err
			}
			if format.value == "json" {
				return render.JSON(a.rawStdout, map[string]any{"mcpServers": map[string]mcpServer{name: entry}})
			}
			var b strings.Builder
			if flags.http != "" {
				fmt.Fprintf(&b, "# 1. Keep the server running:\n%s\n\n", shellJoin(a.serveArgs(flags, true)))
				fmt.Fprintf(&b, "# 2. Register it with Claude Code:\n%s\n\n", add)
			} else {
				fmt.Fprintf(&b, "# Register rvw with Claude Code; it starts the server on stdio:\n%s\n\n", add)
			}
			fmt.Fprintf(&b, "# Or add this to .mcp.json at the project root:\n")
			if err := render.JSON(&b, map[string]any{"mcpServers": map[string]mcpServer{name: entry}}); err != nil {
				return err
			}
			fmt.Fprintf(&b, "\n# Check it with `claude mcp list`, or /mcp inside Claude Code.\n")
			fmt.Fprint(a.stdout, b.String())
			return nil
		},
	}
	flags.register(cmd)
	cmd.Flags().StringVar(&scope, "scope", "user", "claude mcp add scope: local, project, user")
	cmd.Flags().StringVar(&name, "name", "rvw", "server name in Claude Code")
	format.register(cmd, "text", "text", "json")
	return cmd
}

// mcpServer is one server entry of Claude Code's .mcp.json.
type mcpServer struct {
	Type    string   `json:"type"`
	Command string   `json:"command,omitempty"`
	Args    []string `json:"args,omitempty"`
	URL     string   `json:"url,omitempty"`
}

// mcpEntry is the .mcp.json server entry and the `claude mcp add` line for it.
func (a *app) mcpEntry(flags serverFlags, scope, name string) (mcpServer, string, error) {
	if strings.TrimSpace(name) == "" {
		return mcpServer{}, "", usageError("--name must not be empty")
	}
	if flags.http != "" {
		if err := checkLoopback(flags.http); err != nil {
			return mcpServer{}, "", err
		}
		url := mcpURL(flags.http)
		add := shellJoin([]string{"claude", "mcp", "add", "--transport", "http", "--scope", scope, name, url})
		return mcpServer{Type: "http", URL: url}, add, nil
	}
	args := a.serveArgs(flags, false)
	add := shellJoin(append([]string{"claude", "mcp", "add", "--scope", scope, name, "--"}, args...))
	return mcpServer{Type: "stdio", Command: args[0], Args: args[1:]}, add, nil
}

// serveArgs is the `rvw mcp serve` command line that config describes.
func (a *app) serveArgs(flags serverFlags, withHTTP bool) []string {
	args := []string{executable(), "mcp", "serve"}
	if a.dbFlag != "" {
		if db, err := a.dbPath(); err == nil {
			args = append(args, "--db", db)
		}
	}
	if a.workspaceFlag != "" {
		if ws, err := a.workspace(); err == nil {
			args = append(args, "--workspace", ws)
		}
	}
	if lane := laneOf(flags.lane); lane != "" {
		args = append(args, "--lane", lane)
	}
	if author := strings.TrimSpace(flags.author); author != "" {
		args = append(args, "--author", author)
	}
	if withHTTP {
		args = append(args, "--http", flags.http)
	}
	return args
}

// checkLoopback refuses an --http address other programs on the network could
// reach: the server has no authentication, so it must stay on this machine.
func checkLoopback(addr string) error {
	host, _, err := net.SplitHostPort(addr)
	if err != nil {
		return usageError("--http '%s' is not HOST:PORT", addr)
	}
	if host == "localhost" {
		return nil
	}
	if ip, err := netip.ParseAddr(host); err == nil && ip.IsLoopback() {
		return nil
	}
	return usageError("--http must be a loopback address, got '%s'", addr)
}

func mcpURL(addr string) string { return "http://" + addr + mcpPath }

// executable is how to start this binary: "rvw" when PATH finds this very
// file, else its absolute path.
func executable() string {
	self, err := os.Executable()
	if err != nil {
		return prog
	}
	self, _ = filepath.EvalSymlinks(self)
	if found, err := exec.LookPath(prog); err == nil {
		if found, err = filepath.EvalSymlinks(found); err == nil && found == self {
			return prog
		}
	}
	return self
}

// buildVersion is set by the release build (-ldflags -X); go install leaves it
// empty and the module version comes from the build info instead.
var buildVersion string

func version() string {
	if buildVersion != "" {
		return buildVersion
	}
	if info, ok := debug.ReadBuildInfo(); ok && info.Main.Version != "" {
		return info.Main.Version
	}
	return "(devel)"
}

// shellJoin quotes each word that a POSIX shell would split or expand.
func shellJoin(words []string) string {
	quoted := make([]string, len(words))
	for i, w := range words {
		if w != "" && strings.IndexFunc(w, unsafeShellRune) < 0 {
			quoted[i] = w
			continue
		}
		quoted[i] = "'" + strings.ReplaceAll(w, "'", `'\''`) + "'"
	}
	return strings.Join(quoted, " ")
}

func unsafeShellRune(r rune) bool {
	switch {
	case r >= 'a' && r <= 'z', r >= 'A' && r <= 'Z', r >= '0' && r <= '9':
		return false
	}
	return !strings.ContainsRune("-_./:=@,+", r)
}

package review

import (
	"strings"

	"github.com/pmezard/go-difflib/difflib"

	"github.com/gustavofsantos/rvw/internal/gitx"
)

// filetypes maps an extension to the fence language of its code snapshot.
var filetypes = map[string]string{
	".bash": "bash", ".c": "c", ".clj": "clojure", ".cljc": "clojure", ".cljs": "clojure",
	".cpp": "cpp", ".css": "css", ".edn": "clojure", ".ex": "elixir", ".exs": "elixir",
	".go": "go", ".h": "c", ".hs": "haskell", ".html": "html", ".java": "java",
	".js": "javascript", ".json": "json", ".jsx": "javascript", ".lua": "lua",
	".md": "markdown", ".py": "python", ".rb": "ruby", ".rs": "rust", ".scm": "scheme",
	".sh": "bash", ".sql": "sql", ".toml": "toml", ".ts": "typescript", ".tsx": "typescript",
	".vim": "vim", ".yaml": "yaml", ".yml": "yaml", ".zsh": "bash",
}

// snapshotDiff is the unified diff between the file as reviewed and as
// resolved, read back from the git object store. ok is false when either
// version is gone or was never kept.
func snapshotDiff(c Comment) (lines []string, ok bool) {
	if c.FileVersion == "" || c.ResolvedFileVersion == "" {
		return []string{}, false
	}
	before, okBefore := gitx.Blob(c.Workspace, c.FileVersion)
	after, okAfter := gitx.Blob(c.Workspace, c.ResolvedFileVersion)
	if !okBefore || !okAfter {
		return []string{}, false
	}
	text, err := difflib.GetUnifiedDiffString(difflib.UnifiedDiff{
		A:        withNewlines(before),
		B:        withNewlines(after),
		FromFile: "a/" + c.File,
		ToFile:   "b/" + c.File,
		Context:  3,
	})
	if err != nil {
		return []string{}, false
	}
	lines = []string{}
	if text != "" {
		lines = strings.Split(strings.TrimSuffix(text, "\n"), "\n")
	}
	return lines, true
}

// withNewlines splits text into lines that each end in "\n", the shape difflib
// expects; a missing final newline is not reported as a change.
func withNewlines(text string) []string {
	lines := splitSource(strings.ReplaceAll(text, "\r\n", "\n"))
	for i := range lines {
		lines[i] += "\n"
	}
	return lines
}

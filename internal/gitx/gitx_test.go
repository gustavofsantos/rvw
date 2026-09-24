package gitx

import (
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"testing"
)

func repo(t *testing.T) (dir string, write func(rel, content string), git func(args ...string)) {
	t.Helper()
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("needs git")
	}
	dir = t.TempDir()
	write = func(rel, content string) {
		t.Helper()
		path := filepath.Join(dir, rel)
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	git = func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", append([]string{"-C", dir, "-c", "user.name=t", "-c", "user.email=t@t"}, args...)...)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	git("init", "-q")
	return dir, write, git
}

func TestChangesReadsHunksAgainstHEAD(t *testing.T) {
	dir, write, git := repo(t)
	write("a.txt", "1\n2\n3\n4\n5\n6\n")
	write("gone.txt", "x\n")
	write("sub dir/b.txt", "b\n")
	write(".gitignore", "*.log\n")
	git("add", ".")
	git("commit", "-q", "-m", "init")

	write("a.txt", "0\n1\n2\nthree\n5\n6\n+++ not a header\n") // add line 1, 3 and 4 become "three", add line 7
	write("sub dir/b.txt", "")                                 // delete the only line
	if err := os.Remove(filepath.Join(dir, "gone.txt")); err != nil {
		t.Fatal(err)
	}
	write("staged.txt", "s1\ns2\n")
	git("add", "staged.txt")
	write("new.txt", "n1\nn2\nn3")
	write("empty.txt", "")
	write("debug.log", "ignored\n")

	got, err := Changes(dir)
	if err != nil {
		t.Fatal(err)
	}
	want := map[string]Change{
		"a.txt": {Hunks: []Hunk{
			{OldStart: 0, OldCount: 0, NewStart: 1, NewCount: 1},
			{OldStart: 3, OldCount: 2, NewStart: 4, NewCount: 1},
			{OldStart: 6, OldCount: 0, NewStart: 7, NewCount: 1},
		}},
		"sub dir/b.txt": {Hunks: []Hunk{{OldStart: 1, OldCount: 1, NewStart: 0, NewCount: 0}}},
		"staged.txt":    {Added: true, Hunks: []Hunk{{NewStart: 1, NewCount: 2}}},
		"new.txt":       {Added: true, Hunks: []Hunk{{NewStart: 1, NewCount: 3}}},
		"empty.txt":     {Added: true},
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("changes =\n%+v\nwant\n%+v", got, want)
	}

	got, err = Changes(dir, "new.txt")
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 1 || got["new.txt"].Hunks[0].NewCount != 3 {
		t.Fatalf("a pathspec limits the changes: %+v", got)
	}
}

func TestChangesPathsAreRelativeToDir(t *testing.T) {
	dir, write, git := repo(t)
	write("top.txt", "x\n")
	write("pkg/in.txt", "x\n")
	git("add", ".")
	git("commit", "-q", "-m", "init")
	write("top.txt", "y\n")
	write("pkg/in.txt", "y\n")
	write("pkg/new.txt", "n\n")

	got, err := Changes(filepath.Join(dir, "pkg"))
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 2 || got["in.txt"].Hunks == nil || !got["new.txt"].Added {
		t.Fatalf("changes = %+v", got)
	}
}

func TestChangesWithoutACommit(t *testing.T) {
	dir, write, git := repo(t)
	write("a.txt", "1\n2\n")
	git("add", "a.txt")
	got, err := Changes(dir)
	if err != nil {
		t.Fatal(err)
	}
	if want := (Change{Added: true, Hunks: []Hunk{{NewStart: 1, NewCount: 2}}}); !reflect.DeepEqual(got["a.txt"], want) {
		t.Fatalf("a.txt = %+v, want %+v", got["a.txt"], want)
	}
}

func TestChangesOutsideGitFails(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("needs git")
	}
	if _, err := Changes(t.TempDir()); err == nil {
		t.Fatal("not a repository")
	}
}

func TestDiffPathUnquotes(t *testing.T) {
	for in, want := range map[string]string{
		"b/a.txt":             "a.txt",
		"b/with space.txt\t":  "with space.txt",
		`"b/tab\there.txt"`:   "tab\there.txt",
		`"b/caf\303\251.txt"`: "café.txt",
		"/dev/null":           "",
	} {
		if got := diffPath(in); got != want {
			t.Errorf("diffPath(%q) = %q, want %q", in, got, want)
		}
	}
}

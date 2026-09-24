package review_test

import (
	"reflect"
	"slices"
	"testing"

	"github.com/google/jsonschema-go/jsonschema"
	"github.com/gustavofsantos/rvw/internal/review"
)

// Every operation's input and output must infer a JSON schema the way the MCP
// go-sdk does when it registers a tool, so exposing an operation over MCP
// stays a matter of wiring. Outputs must be objects to be structured content.
func TestOperationTypesInferMCPSchemas(t *testing.T) {
	types := []any{
		review.AddInput{}, review.Comment{},
		review.SubmitInput{}, review.Review{},
		review.QueryInput{}, review.ListOutput{}, review.CountOutput{},
		review.PullInput{}, review.PullOutput{},
		review.ResolveInput{}, review.EditInput{},
		review.GetInput{}, review.Evidence{}, review.ReviewSheet{},
		review.SheetsInput{}, review.SheetsOutput{},
		review.DropInput{}, review.DropOutput{},
		review.ClearInput{}, review.ClearOutput{},
		review.WorkspacesInput{}, review.WorkspacesOutput{},
	}
	for _, v := range types {
		typ := reflect.TypeOf(v)
		s, err := jsonschema.ForType(typ, nil)
		if err != nil {
			t.Errorf("%s: %v", typ.Name(), err)
			continue
		}
		if s.Type != "object" {
			t.Errorf("%s: schema type %q, want object", typ.Name(), s.Type)
		}
	}

	// Required-ness follows omitempty: what an agent must send is exactly what
	// the service cannot default.
	s, _ := jsonschema.For[review.AddInput](nil)
	slices.Sort(s.Required)
	if want := []string{"comment", "file", "start_line", "workspace"}; !slices.Equal(s.Required, want) {
		t.Errorf("AddInput required = %v, want %v", s.Required, want)
	}
}

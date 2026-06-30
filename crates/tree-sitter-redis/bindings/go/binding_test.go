package tree_sitter_redis_test

import (
	"testing"

	tree_sitter "github.com/tree-sitter/go-tree-sitter"
	tree_sitter_redis "github.com/tree-sitter/tree-sitter-redis/bindings/go"
)

func TestCanLoadGrammar(t *testing.T) {
	language := tree_sitter.NewLanguage(tree_sitter_redis.Language())
	if language == nil {
		t.Errorf("Error loading Redis grammar")
	}
}

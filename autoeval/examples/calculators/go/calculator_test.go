package calculator

import "testing"

func TestZero(t *testing.T) { if Add(0, 0) != 0 { t.Fatal("zero") } }
func TestPositive(t *testing.T) { if Add(2, 3) != 5 { t.Fatal("positive sum") } }
func TestNegative(t *testing.T) { if Add(-2, -3) != -5 { t.Fatal("negative sum") } }

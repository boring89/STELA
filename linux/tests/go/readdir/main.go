// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

// readdir: os.ReadDir, a file written and read back, os.Stat.
package main

import (
	"fmt"
	"os"
	"strings"
)

func main() {
	dir := "."
	if len(os.Args) > 1 {
		dir = os.Args[1]
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		fmt.Printf("go: %v\n", err)
		os.Exit(1)
	}
	var names []string
	for _, e := range entries {
		names = append(names, e.Name())
	}
	fmt.Printf("go: %s has %d entries: %s\n", dir, len(entries), strings.Join(names, " "))
	path := "/tmp/go-test.txt"
	if err := os.WriteFile(path, []byte("written by Go\n"), 0o644); err != nil {
		fmt.Printf("go: %v\n", err)
		os.Exit(1)
	}
	data, err := os.ReadFile(path)
	st, err2 := os.Stat(path)
	if err != nil || err2 != nil {
		fmt.Printf("go: %v %v\n", err, err2)
		os.Exit(1)
	}
	fmt.Printf("go: read back %q, size %d\n", data, st.Size())
}

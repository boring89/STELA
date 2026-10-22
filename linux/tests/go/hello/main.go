// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

// hello: a Go program on STELA's Linux personality (P14 K9,
// docs/linux.md §17): arguments, the environment, the exit code.
package main

import (
	"fmt"
	"os"
	"runtime"
)

func main() {
	fmt.Printf("go: hello from %s/%s (%s), %d args\n", runtime.GOOS, runtime.GOARCH, runtime.Version(), len(os.Args))
	for i, a := range os.Args[1:] {
		fmt.Printf("go: argv[%d] = %s\n", i+1, a)
	}
	host, err := os.Hostname()
	fmt.Printf("go: hostname %q (%v)\n", host, err)
	os.Exit(7)
}

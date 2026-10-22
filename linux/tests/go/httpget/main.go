// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

// httpget: net/http's client (the netpoller: epoll, nonblocking
// sockets) fetches a URL and prints the body.
package main

import (
	"fmt"
	"io"
	"net/http"
	"os"
	"time"
)

func main() {
	if len(os.Args) < 2 {
		fmt.Println("go: usage: httpget URL")
		os.Exit(2)
	}
	c := http.Client{Timeout: 20 * time.Second}
	r, err := c.Get(os.Args[1])
	if err != nil {
		fmt.Printf("go: %v\n", err)
		os.Exit(1)
	}
	defer r.Body.Close()
	body, err := io.ReadAll(r.Body)
	if err != nil {
		fmt.Printf("go: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("go: HTTP %d, %d bytes: %s", r.StatusCode, len(body), body)
}

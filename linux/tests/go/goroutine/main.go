// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

// goroutine: goroutines on several threads, channels, a mutex, timers,
// and preemption: a goroutine in a loop without calls must be stopped
// by the runtime's SIGURG (signals on the alternate stack, P14 K9).
package main

import (
	"fmt"
	"runtime"
	"sync"
	"sync/atomic"
	"time"
)

func main() {
	runtime.GOMAXPROCS(4)
	results := make(chan int)
	var wg sync.WaitGroup
	var mu sync.Mutex
	total := 0
	for i := 1; i <= 8; i++ {
		wg.Add(1)
		go func(n int) {
			defer wg.Done()
			sum := 0
			for k := 0; k < n*100000; k++ {
				sum += k % 7
			}
			mu.Lock()
			total += n
			mu.Unlock()
			results <- sum
		}(i)
	}
	go func() { wg.Wait(); close(results) }()
	count := 0
	for range results {
		count++
	}
	fmt.Printf("go: %d goroutines done, total %d\n", count, total)

	// Preemption: with one P, a spinning goroutine must not keep the
	// timer from firing.
	runtime.GOMAXPROCS(1)
	var stop atomic.Bool
	go func() {
		// No function calls: only an asynchronous preemption stops it.
		for !stop.Load() {
		}
	}()
	start := time.Now()
	time.Sleep(50 * time.Millisecond)
	stop.Store(true)
	fmt.Printf("go: preempted after %d ms\n", time.Since(start).Milliseconds()/10*10)
	select {
	case <-time.After(20 * time.Millisecond):
		fmt.Println("go: timer OK")
	}
}

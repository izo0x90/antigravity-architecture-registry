package main

import "example.com/app/store"

func shout(s string) string {
	return s + "!"
}

func keep(st *store.Store, msg string) {
	_ = st
	_ = msg
}

func main() {
	st := store.New()
	msg := shout("hi")
	keep(st, msg)
}

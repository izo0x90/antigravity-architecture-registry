package main

import st "example.com/app/store"

func shout(s string) string {
	return s + "!"
}

func keep(s *st.Store, msg string) {
	_ = s
	_ = msg
}

func main() {
	s := st.New()
	msg := shout("hi")
	keep(s, msg)
}

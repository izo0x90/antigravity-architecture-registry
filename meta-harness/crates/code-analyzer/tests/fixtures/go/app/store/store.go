package store

type Saver interface {
	Save(item string) error
}

type Store struct {
	items []string
}

func New() *Store {
	return &Store{}
}

func (s *Store) Save(item string) error {
	s.items = append(s.items, item)
	return s.check()
}

func (s *Store) check() error {
	return nil
}

func Later() int {
	panic("TODO")
}

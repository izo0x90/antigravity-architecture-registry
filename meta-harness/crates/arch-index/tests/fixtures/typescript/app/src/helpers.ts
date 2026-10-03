export interface Formatter {
  format(text: string): string;
}

export class Base {
  describe(): string {
    return "base";
  }
}

export function clean(s: string): string {
  return s.trim();
}

export function load() {
  return " data ";
}

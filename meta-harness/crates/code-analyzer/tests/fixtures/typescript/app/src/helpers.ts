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

export const shout = (s: string): string => s.toUpperCase();

export function load() {
  return " data ";
}

// Transcript content is untrusted display data. Keep only printable characters
// plus line feeds before it reaches terminal output.
export function sanitizeTerminalText(value) {
  return String(value ?? '').replace(/[\u0000-\u0009\u000B-\u001F\u007F-\u009F]/g, '');
}

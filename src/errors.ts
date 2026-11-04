export class ToolFailure extends Error {
  readonly code: string;
  /** Structured facts the helper attached to the error, if any. */
  readonly data: Record<string, unknown> | undefined;

  constructor(code: string, message: string, data?: Record<string, unknown>) {
    super(message);
    this.name = "ToolFailure";
    this.code = code;
    this.data = data;
  }
}

export function toolError(code: string, message: string): {
  isError: true;
  content: [{ type: "text"; text: string }];
} {
  return {
    isError: true,
    content: [{ type: "text", text: `${code}\n${message}` }],
  };
}

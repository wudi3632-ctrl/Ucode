export class AppError extends Error {
  constructor(code, message, status = 400, details = undefined) {
    super(message);
    this.code = code;
    this.status = status;
    this.details = details;
  }
}

export const errorBody = (error, requestId) => ({
  error: {
    code: error.code || "internal_error",
    message: error instanceof AppError ? error.message : "Internal error",
    ...(error.details === undefined ? {} : { details: error.details }),
  },
  requestId,
});

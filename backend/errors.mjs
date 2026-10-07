export class ApiError extends Error {
  constructor(status, code, message) {
    super(message)
    this.status = status
    this.code = code
  }
}
export function reject(status, code, message) { throw new ApiError(status, code, message) }
export function invalid(message) { reject(400, 'VALIDATION_ERROR', message) }
export function notFound() { reject(404, 'RESOURCE_NOT_FOUND', '工单不存在或不在当前授权范围内') }

import { v7, validate, version } from 'uuid'

export const newId = () => v7()
export const validUUID = value => typeof value === 'string' && validate(value) && version(value) === 7

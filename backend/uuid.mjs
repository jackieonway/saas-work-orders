import { v7, validate, version } from 'uuid'

// Entity and trace identifiers share the same RFC 9562 boundary. Credentials
// and other random secrets must continue using their dedicated CSPRNG APIs.
export const newId = () => v7()
export const validId = value => typeof value === 'string' && validate(value) && version(value) === 7

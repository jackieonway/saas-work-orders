import './app.css'
import { createLifecycle } from './app.mjs'
import { createQiankunLifecycle } from './qiankun-lifecycle.mjs'

const lifecycle = createQiankunLifecycle(createLifecycle, __QIANKUN_PUBLIC_CONFIG__)
export const { bootstrap, mount, update, unmount } = lifecycle

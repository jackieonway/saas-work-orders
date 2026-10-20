import './app.css'
import { APP_CODE } from './domain.mjs'
import { createLazyLifecycle } from './lifecycle.mjs'

const config = window.__WORK_ORDERS_CONFIG__
const container = document.querySelector('#app')
if (!config?.allowedHostOrigins?.length || !window.MicroIframe?.connect) {
  container.textContent = '工单应用尚未配置宿主连接，请联系平台管理员。'
} else {
  // Register the SDK listener before iframe load transfers its single port.
  // Awaiting a dynamic module at the top level does not provide that ordering.
  let bridge
  const lifecycle = createLazyLifecycle(() => import('./app.mjs'), () => bridge)
  bridge = window.MicroIframe.connect({ appCode: APP_CODE, allowedHostOrigins: config.allowedHostOrigins,
    container, lifecycle })
}

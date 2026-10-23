/** qiankun metadata and host services stay outside the strict MicroProps contract. */
export function createQiankunLifecycle(createApplication, options = {}) {
  let application, services
  let queue = Promise.resolve()
  const enqueue = action => {
    const result = queue.then(action)
    queue = result.catch(() => {})
    return result
  }
  const platform = props => {
    if (!props?.platform?.signal) throw new TypeError('qiankun requires props.platform with the platform context')
    return props.platform
  }
  const hostServices = {
    downloadCsv(payload) {
      if (typeof services?.downloadCsv !== 'function') throw new Error('平台未提供已授权的 CSV 下载服务')
      return services.downloadCsv(payload)
    },
  }
  return {
    async bootstrap() {},
    mount(props) {
      return enqueue(async () => {
        const context = platform(props)
        if (context.signal.aborted) return
        if (application) throw new Error('微应用已挂载')
        if (options.allowedHostOrigins && !options.allowedHostOrigins.includes(window.location.origin)) throw new Error('当前宿主来源未获准')
        const target = props.container?.querySelector('[data-work-orders-mount]')
        if (!target) throw new Error('缺少工单微应用挂载容器')
        services = props.services
        application = createApplication(hostServices)
        try { await application.mount(target, context) }
        catch (error) { await application.unmount(); application = undefined; services = undefined; throw error }
      })
    },
    update(props) {
      return enqueue(async () => {
        const context = platform(props)
        if (!application || context.signal.aborted) return
        services = props.services
        await application.update?.(context)
      })
    },
    unmount() {
      return enqueue(async () => {
        const previous = application
        application = undefined; services = undefined
        await previous?.unmount()
      })
    },
  }
}

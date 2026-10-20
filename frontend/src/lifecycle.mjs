/** Keep the synchronous SDK handshake separate from the lazy Vue application. */
export function createLazyLifecycle(loadModule, getBridge) {
  let pending, application, target, stopped = false, unmounted = false, mounted = false
  function load() {
    if (!pending) pending = Promise.resolve().then(loadModule).then(module => {
      if (stopped) return null
      application = module.createLifecycle(getBridge())
      return application
    })
    return pending
  }
  return {
    async mount(container, props) {
      if (stopped) return
      target = container
      if (props.signal.aborted) return
      const lifecycle = await load()
      // A different context or page disposal may have cancelled while importing.
      if (!lifecycle || stopped || props.signal.aborted) return
      mounted = true
      return lifecycle.mount(target, props)
    },
    async update(props) {
      if (stopped || props.signal.aborted) return
      const lifecycle = await load()
      if (!lifecycle || stopped || props.signal.aborted) return
      // The SDK marks its lifecycle mounted before awaiting this wrapper. If
      // the first context expired during import, its next callback is update;
      // that fresh context must still perform the first actual Vue mount.
      if (!mounted) {
        mounted = true
        return lifecycle.mount(target, props)
      }
      return lifecycle.update(props)
    },
    async unmount() {
      stopped = true
      // Disposal can happen before the module arrives; do not create a Vue app
      // then. A later mount callback also observes the stopped flag above.
      if (!application || unmounted) return
      unmounted = true
      return application.unmount()
    },
  }
}

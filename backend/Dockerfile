FROM node:24.3.0-alpine
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --omit=dev --ignore-scripts --no-audit --no-fund
COPY --chown=node:node *.mjs ./
COPY --chown=node:node migrations/ ./migrations/
USER node
ENV NODE_ENV=production WORK_ORDERS_HOST=0.0.0.0 WORK_ORDERS_PORT=3004
EXPOSE 3004
HEALTHCHECK --interval=20s --timeout=5s --start-period=20s --retries=3 CMD node --input-type=module -e "const r=await fetch('http://127.0.0.1:'+process.env.WORK_ORDERS_PORT+'/work-orders-api/health',{signal:AbortSignal.timeout(3000)});if(!r.ok)process.exit(1)"
CMD ["node", "index.mjs"]

FROM node:22-alpine AS deps
WORKDIR /app
COPY package.json ./
RUN npm install

FROM node:22-alpine AS builder
WORKDIR /app
COPY --from=deps /app/node_modules ./node_modules
COPY . .
ENV NASIKO_API_URL=http://nasiko-backend:8000/api/v1
RUN npm run build

FROM node:22-alpine AS runner
WORKDIR /app
ENV NODE_ENV=production
ENV NASIKO_API_URL=http://nasiko-backend:8000/api/v1
COPY --from=builder /app/public ./public
COPY --from=builder /app/.next/standalone ./
COPY --from=builder /app/.next/static ./.next/static
EXPOSE 4001
ENV PORT=4001
ENV HOSTNAME=0.0.0.0
CMD ["node", "server.js"]

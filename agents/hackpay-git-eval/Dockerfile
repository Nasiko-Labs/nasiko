FROM node:20-alpine
WORKDIR /app
COPY package.json AgentCard.json ./
RUN npm install --omit=dev
COPY server.js ./
COPY lib ./lib
ENV PORT=8000
ENV NODE_ENV=production
# Local Nasiko compose: write evaluate runs into chat_sessions for sessions.html
ENV NASIKO_SESSION_HISTORY=true
ENV NASIKO_HISTORY_DATABASE_URL=postgres://nasiko:nasiko@postgres:5432/nasiko_dev
ENV NASIKO_AGENT_NAME=hackpay-git-eval
ENV NASIKO_CP_URL=http://server:8080
EXPOSE 8000
CMD ["node", "server.js"]

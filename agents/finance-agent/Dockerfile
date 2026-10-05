FROM node:18-alpine

WORKDIR /app

COPY package.json ./
RUN npm install

COPY index.js AgentCard.json ./

EXPOSE 3000

CMD ["npm", "start"]

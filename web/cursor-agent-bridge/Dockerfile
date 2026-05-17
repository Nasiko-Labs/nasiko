FROM node:22-alpine

WORKDIR /bridge

COPY package.json ./
RUN npm install --omit=dev

COPY server.mjs ./

EXPOSE 8787

CMD ["npm", "start"]

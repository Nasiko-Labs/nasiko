FROM node:20-alpine AS build
WORKDIR /app
ARG VITE_BASE_PATH=./
ENV VITE_BASE_PATH=$VITE_BASE_PATH

COPY package.json package-lock.json ./
RUN npm ci

COPY index.html vite.config.ts tsconfig.json tsconfig.node.json ./
COPY src ./src
COPY nginx.conf ./nginx.conf

# Vite bundles TS; skip separate tsc step so devDependencies resolve reliably in Alpine.
RUN ./node_modules/.bin/vite build

FROM nginx:alpine
COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /app/dist /usr/share/nginx/html
EXPOSE 80

FROM python:3.13-alpine
RUN apk add --no-cache openssh-client \
    && addgroup -g 1000 refresh \
    && adduser -D -u 1000 -G refresh refresh
WORKDIR /app
COPY tools/cookie_refresh_service.py tools/export_chrome_cookies.py /app/
ENV PYTHONDONTWRITEBYTECODE=1 HOME=/tmp
USER 1000:1000
ENTRYPOINT ["python3", "/app/cookie_refresh_service.py"]

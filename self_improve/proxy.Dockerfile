FROM python:3.13-alpine
RUN addgroup -g 1000 proxy && adduser -D -u 1000 -G proxy proxy
COPY proxy.py /app/proxy.py
USER 1000:1000
ENTRYPOINT ["python3", "/app/proxy.py"]

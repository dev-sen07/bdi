FROM python:3.11-slim

WORKDIR /app

COPY services/eval-runner/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY services/eval-runner/run_eval.py .

CMD ["python", "run_eval.py"]

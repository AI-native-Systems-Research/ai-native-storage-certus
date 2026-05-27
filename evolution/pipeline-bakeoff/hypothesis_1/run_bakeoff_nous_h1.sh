#!/bin/bash
export OPENAI_API_KEY=$(cat /tmp/.bakeoff_key)
export OPENAI_BASE_URL="https://ete-litellm.ai-models.vpc-int.res.ibm.com"
cd /home/nara/certus/ai-native-storage-certus
exec python3 evolution/pipeline-bakeoff/run_bakeoff.py \
    --iterations 10 \
    --frameworks nous \
    2>&1 | tee evolution/pipeline-bakeoff/results/bakeoff-nous-h1-rerun.log

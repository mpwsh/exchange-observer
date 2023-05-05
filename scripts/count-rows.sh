#!/bin/bash
if [ -z $2 ];then
echo "Usage ./count-rows-table.sh <keyspace> <table>"
else
docker exec scylla-node1 cqlsh -e "COPY $1.$2 to '/dev/null' with numprocesses=8 AND PAGESIZE=5000 AND PAGETIMEOUT=4000 AND MAXATTEMPTS=50;"
fi

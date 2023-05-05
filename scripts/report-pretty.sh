#!/bin/bash
hash=$1
cqlsh(){
 docker exec scylla-node1 cqlsh "$@"
}
while true;do
        clear
        echo "Possitives"
        cqlsh -e "select count(*) from okx.reports where change >= 0.04 allow filtering;"
        echo "Negatives"
        cqlsh -e "select count(*) from okx.reports where change <= -0.1 allow filtering;"
        echo "Win Reports"
        #strategy | ts | buy_price | change | earnings | highest | highest_elapsed | instid | lowest | lowest_elapsed | reason | round_id | sell_price | time_left
        cqlsh -e "select round_id, instid, change, earnings, highest, highest_elapsed, lowest, lowest_elapsed, time_left, reason, ts from okx.reports WHERE strategy='$hash' allow filtering;"
        echo "earnings (all runs)"
        cqlsh -e "select sum(earnings) from okx.reports WHERE strategy='$hash' ALLOW FILTERING;"
        sleep 20
done

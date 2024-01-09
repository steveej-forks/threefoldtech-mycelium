set NATNET 172.16.0.0/16
set NUMOFNS 32

function IPN
    sudo ip net $argv
end

function IPL
    sudo ip link $argv
end

function IPA
    sudo ip addr add $argv
end

set peers tcp://146.185.93.83:9651 quic://83.231.240.31:9651 quic://185.206.122.71:9651 tcp://[2a04:f340:c0:71:28cc:b2ff:fe63:dd1c]:9651 tcp://[2001:728:1000:402:78d3:cdff:fe63:e07e]:9651 quic://[2a10:b600:1:0:ec4:7aff:fe30:8235]:9651

function IPNA
    set name $argv[1]
    set -e argv[1]
    sudo ip -n $name addr add $argv
end

function IPNL
    set name $argv[1]
    set -e argv[1]
    sudo ip -n $name link $argv
end

function IPNR
    set name $argv[1]
    set defrtr (string replace -r '/24$' '' $argv[2])
    set -e argv[1]
    set -e argv[2]
    sudo ip -n $name route add default via $defrtr
end

function createns
    set iname $argv[1]
    set in_ip $argv[2]
    set out_ip $argv[3]
    set name n-$iname
    IPN add $name
    IPL add in_$iname type veth peer name out_$iname
    IPL set in_$iname netns $name
    IPNL $name set lo up
    IPNL $name set in_$iname up
    IPL set out_$iname up
    IPNA $name $in_ip dev in_$iname
    IPA $out_ip dev out_$iname
    IPNR $name $out_ip
    nohup sudo ip netns exec $name ./mycelium --key-file $name.bin --api-addr (string replace -r '/24$' '' $in_ip):8989 --peers tcp://(string replace -r '/24$' '' $out_ip):9651 > $iname.out &
end

function dropns
    set iname $argv[1]
    set name n-$iname
    IPL del out_$iname
    IPN del $name
end

function doit
    nohup sudo ./mycelium --key-file host.bin --api-addr 127.0.0.1:8989 --peers $peers > host.out &
    for i in (seq 1 $NUMOFNS)
        createns $i 172.16.$i.2/24 172.16.$i.1/24
    end
end

function dropit
    sudo pkill -9 mycelium
    for i in (seq 1 $NUMOFNS)
        dropns $i
    end
end

function cleanit
    dropit
    sudo rm ./*.bin
    sudo rm ./*.out
end

function showit
    sudo killall -USR1 mycelium
end

function getmycelium
    wget https://github.com/threefoldtech/mycelium/releases/latest/download/mycelium-x86_64-unknown-linux-musl.tar.gz \
        -O- | gunzip -c | tar xvf - -C $PWD
end

